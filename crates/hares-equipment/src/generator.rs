//! Generator equipment models: gas generators and fuel cells.
//!
//! Gas generators share identical combustion physics (efficiency model, ramp-rate, self-consumption,
//! combined heat and power). Fuel cells diverge: DC stack power passes through an inverter
//! (DC→AC conversion with configurable efficiency), and stack cooling heat is tracked separately
//! via the EnergyPlus stack cooler polynomial. A single `Generator` struct handles all logic with
//! conditional branching on `GeneratorKind`.
//! `GasGenerator` and `FuelCell` are newtypes that forward via `delegate_equipment!`.
//!
//! Physics overview:
//!   eta = EfficiencyModel::evaluate(capacity_ratio)  -- constant, curve, or quadratic
//!   P_fuel = P_electric / eta
//!   Q_thermal = P_fuel * eta_thermal  (CHP only)
//!   Q_flue = P_fuel - P_electric - Q_thermal  (residual loss)
//!   Constraint: eta_electric_rated + eta_thermal <= 1.0
//!
//! OCHRE reference: `ochre/Equipment/Generator.py`
//! Improvements over OCHRE:
//!   - CHP thermal and fluid ports are fully implemented (OCHRE has them stubbed)
//!   - Self-consumption reads accumulated Stage 1 PortSlots (OCHRE uses schedule injection)
//!   - Ramp-rate limiting only constrains power increases (matches OCHRE; decreases are instant)

use std::borrow::Cow;
use std::time::Duration;

use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FluidType, FuelPower, FuelType, HaresError, LoopId, OperatingMode,
    PortContribution, PortDeclaration, PortSlots, Telemetry, TelemetryField, ThermalCategory,
    ZoneId, telemetry_keys as tk,
};
use serde::{Deserialize, Serialize};

use hares_physics::constants::CP_LIQUID_WATER_J_KG_K;
use hares_physics::units::{power_kw_to_w, power_w_to_kw};

use crate::config::EquipmentTypedConfig;
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

// ---------------------------------------------------------------------------
// Typed config
// ---------------------------------------------------------------------------

/// Typed configuration for gas generator and fuel cell equipment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub fuel_type: Option<FuelType>,
    pub rated_power_kw: f64,
    pub eta_electric: Option<f64>,
    /// DEPRECATED: lumped thermal recovery efficiency. Replaced by per-stream
    /// eta_jacket_water / eta_lube_oil / eta_exhaust. When only eta_thermal
    /// is provided and the per-stream fields are absent, it is distributed
    /// using engineering-estimate split fractions: jacket ~30 %, lube ~10 %,
    /// exhaust ~60 %. EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine:
    /// per-stream heat recovery fractions are manufacturer-supplied PLR-dependent
    /// curves, not fixed values.
    pub eta_thermal: Option<f64>,
    /// Recoverable jacket water heat fraction at ~90°C.
    /// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine:
    /// jacket water heat recovery fraction is a manufacturer-supplied quadratic
    /// PLR curve (b₁ + b₂·PLR + b₃·PLR²).
    pub eta_jacket_water: Option<f64>,
    /// Recoverable lube oil heat fraction at ~85°C.
    /// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine:
    /// lube oil heat recovery fraction is a manufacturer-supplied quadratic
    /// PLR curve (c₁ + c₂·PLR + c₃·PLR²).
    pub eta_lube_oil: Option<f64>,
    /// Recoverable exhaust heat fraction at ~400–500°C.
    /// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine:
    /// exhaust heat recovery fraction is a manufacturer-supplied quadratic
    /// PLR curve (d₁ + d₂·PLR + d₃·PLR²).
    pub eta_exhaust: Option<f64>,
    pub efficiency_type: Option<String>,
    pub efficiency_curve_points: Option<Vec<GeneratorEfficiencyCurvePoint>>,
    pub delta_kw_per_s: Option<f64>,
    pub capacity_min_kw: Option<f64>,
    pub grid_import_limit_kw: Option<f64>,
    pub export_limit_kw: Option<f64>,
    /// CHP fluid loop ID (non-zero when thermal recovery is enabled)
    pub loop_id: Option<u16>,
    pub flow_rate_kg_s: Option<f64>,
    pub supply_temp_c: Option<f64>,
    pub return_temp_c: Option<f64>,
    /// FuelCell DC-to-AC inverter efficiency [0, 1].
    /// Ignored for GasGenerator. Default: 0.95 for FuelCell (EnergyPlus typical).
    pub inverter_efficiency: Option<f64>,
    /// FuelCell stack operating temperature (°C).
    /// EnergyPlus FuelCellElectricGenerator.cc:1859 — TstackActual.
    pub stack_temp_c: Option<f64>,
    /// FuelCell stack cooler polynomial coefficient r0 (dimensionless offset).
    /// EnergyPlus FuelCellElectricGenerator.cc:1862 — qs_cool polynomial.
    pub stack_cooler_r0: Option<f64>,
    /// FuelCell stack cooler polynomial coefficient r1 (1/K or 1/°C).
    pub stack_cooler_r1: Option<f64>,
    /// FuelCell stack cooler polynomial coefficient r2 (1/W).
    pub stack_cooler_r2: Option<f64>,
    /// FuelCell stack cooler polynomial coefficient r3 (1/W^2).
    pub stack_cooler_r3: Option<f64>,
    /// FuelCell stack cooler nominal temperature (°C) for offset reference.
    /// EnergyPlus FuelCellElectricGenerator.cc:1862 — TstackNom.
    pub stack_nominal_temp_c: Option<f64>,
    /// Maximum heat recovery fluid temperature (°C).
    /// When set, thermal output is capped so the loop return temperature plus
    /// the temperature rise from heat recovery does not exceed this value.
    /// EnergyPlus ICEngineElectricGenerator.cc:768 — HeatRecMaxTemp.
    pub heat_rec_max_temp_c: Option<f64>,
}

impl EquipmentTypedConfig for GeneratorConfig {
    fn equipment_type_name() -> &'static str {
        "Generator"
    }
}

/// Piecewise-linear generator efficiency curve point.
///
/// `capacity_ratio` is normalized electric output in `[0, 1]`.
/// `efficiency_ratio` scales `eta_electric` at that operating point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorEfficiencyCurvePoint {
    pub capacity_ratio: f64,
    pub efficiency_ratio: f64,
}

impl GeneratorConfig {
    /// Validate fields for physical plausibility.
    pub fn validate(&self) -> crate::Result<()> {
        if !self.rated_power_kw.is_finite() || self.rated_power_kw <= 0.0 {
            return Err(HaresError::Equipment(
                "generator rated_power_kw must be finite and > 0".to_string(),
            ));
        }
        for (name, val) in [
            ("eta_electric", self.eta_electric),
            ("eta_thermal", self.eta_thermal),
            ("eta_jacket_water", self.eta_jacket_water),
            ("eta_lube_oil", self.eta_lube_oil),
            ("eta_exhaust", self.eta_exhaust),
        ] {
            if let Some(v) = val {
                if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                    return Err(HaresError::Equipment(format!(
                        "generator {name} must be finite and within [0, 1]"
                    )));
                }
            }
        }
        // Validate legacy combined eta_thermal + eta_electric sum constraint.
        if let (Some(eta_e), Some(eta_t)) = (self.eta_electric, self.eta_thermal) {
            if eta_e + eta_t > 1.0 + f64::EPSILON {
                return Err(HaresError::Equipment(
                    "generator eta_electric + eta_thermal must not exceed 1.0".to_string(),
                ));
            }
        }
        // Validate per-stream eta sum against eta_electric when per-stream fields are used.
        let per_stream_sum = self.eta_jacket_water.unwrap_or(0.0)
            + self.eta_lube_oil.unwrap_or(0.0)
            + self.eta_exhaust.unwrap_or(0.0);
        if per_stream_sum > 0.0 {
            if let Some(eta_e) = self.eta_electric {
                if eta_e + per_stream_sum > 1.0 + f64::EPSILON {
                    return Err(HaresError::Equipment(
                        "generator eta_electric + per-stream heat recovery sum must not exceed 1.0"
                            .to_string(),
                    ));
                }
            }
        }
        if let Some(ramp) = self.delta_kw_per_s {
            if !ramp.is_finite() || ramp <= 0.0 {
                return Err(HaresError::Equipment(
                    "generator delta_kw_per_s must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(efficiency_type) = self.efficiency_type.as_deref() {
            match efficiency_type {
                "constant" | "curve" | "quadratic" => {}
                _ => {
                    return Err(HaresError::Equipment(format!(
                        "unrecognised generator efficiency_type '{efficiency_type}'; \
                         expected one of: constant, curve, quadratic"
                    )));
                }
            }
        }
        if let Some(points) = self.efficiency_curve_points.as_deref() {
            EfficiencyModel::validate_curve_points(points)?;
        }
        if let Some(min_kw) = self.capacity_min_kw {
            if !min_kw.is_finite() || min_kw < 0.0 || min_kw > self.rated_power_kw {
                return Err(HaresError::Equipment(
                    "generator capacity_min_kw must be finite, >= 0, and <= rated_power_kw"
                        .to_string(),
                ));
            }
        }
        if let Some(zone) = self.zone_id
            && zone == 0
        {
            return Err(HaresError::Equipment(
                "generator zone_id must be non-zero when provided".to_string(),
            ));
        }
        if let Some(inv_eff) = self.inverter_efficiency {
            if !inv_eff.is_finite() || inv_eff <= 0.0 || inv_eff > 1.0 {
                return Err(HaresError::Equipment(
                    "generator inverter_efficiency must be finite and in (0, 1]".to_string(),
                ));
            }
        }
        if let Some(st) = self.stack_temp_c {
            if !st.is_finite() || st < 0.0 {
                return Err(HaresError::Equipment(
                    "generator stack_temp_c must be finite and >= 0".to_string(),
                ));
            }
        }
        for (name, val) in [
            ("stack_cooler_r0", self.stack_cooler_r0),
            ("stack_cooler_r1", self.stack_cooler_r1),
            ("stack_cooler_r2", self.stack_cooler_r2),
            ("stack_cooler_r3", self.stack_cooler_r3),
        ] {
            if let Some(v) = val {
                if !v.is_finite() {
                    return Err(HaresError::Equipment(format!(
                        "generator {name} must be finite"
                    )));
                }
            }
        }
        if let Some(snt) = self.stack_nominal_temp_c {
            if !snt.is_finite() || snt < 0.0 {
                return Err(HaresError::Equipment(
                    "generator stack_nominal_temp_c must be finite and >= 0".to_string(),
                ));
            }
        }
        if let Some(hrmt) = self.heat_rec_max_temp_c {
            if !hrmt.is_finite() || hrmt <= 0.0 {
                return Err(HaresError::Equipment(
                    "generator heat_rec_max_temp_c must be finite and > 0".to_string(),
                ));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Config keys
// ---------------------------------------------------------------------------

#[cfg(test)]
use crate::config::{KEY_EQUIPMENT_ID, KEY_ZONE_ID};
#[cfg(test)]
const KEY_RATED_POWER_KW: &str = "rated_power_kw";
#[cfg(test)]
const KEY_CAPACITY_MIN_KW: &str = "capacity_min_kw";
#[cfg(test)]
const KEY_ETA_ELECTRIC: &str = "eta_electric";
#[cfg(test)]
const KEY_ETA_THERMAL: &str = "eta_thermal";
#[cfg(test)]
const KEY_ETA_JACKET_WATER: &str = "eta_jacket_water";
#[cfg(test)]
const KEY_ETA_LUBE_OIL: &str = "eta_lube_oil";
#[cfg(test)]
const KEY_ETA_EXHAUST: &str = "eta_exhaust";
#[cfg(test)]
const KEY_EFFICIENCY_TYPE: &str = "efficiency_type";
#[cfg(test)]
const KEY_DELTA_KW_PER_S: &str = "delta_kw_per_s";
#[cfg(test)]
const KEY_GRID_IMPORT_LIMIT_KW: &str = "grid_import_limit_kw";
#[cfg(test)]
const KEY_EXPORT_LIMIT_KW: &str = "export_limit_kw";
#[cfg(test)]
const KEY_LOOP_ID: &str = "loop_id";
#[cfg(test)]
const KEY_FLOW_RATE_KG_S: &str = "flow_rate_kg_s";
#[cfg(test)]
const KEY_SUPPLY_TEMP_C: &str = "supply_temp_c";
#[cfg(test)]
const KEY_RETURN_TEMP_C: &str = "return_temp_c";
#[cfg(test)]
const KEY_INVERTER_EFFICIENCY: &str = "inverter_efficiency";
#[cfg(test)]
const KEY_STACK_TEMP_C: &str = "stack_temp_c";
#[cfg(test)]
const KEY_STACK_COOLER_R0: &str = "stack_cooler_r0";
#[cfg(test)]
const KEY_STACK_COOLER_R1: &str = "stack_cooler_r1";
#[cfg(test)]
const KEY_STACK_COOLER_R2: &str = "stack_cooler_r2";
#[cfg(test)]
const KEY_STACK_COOLER_R3: &str = "stack_cooler_r3";
#[cfg(test)]
const KEY_STACK_NOMINAL_TEMP_C: &str = "stack_nominal_temp_c";
#[cfg(test)]
const KEY_HEAT_REC_MAX_TEMP_C: &str = "heat_rec_max_temp_c";

// ---------------------------------------------------------------------------
// Physical defaults
// ---------------------------------------------------------------------------

/// Typical spark-ignited natural gas generator electrical efficiency at rated load.
/// Generac/Briggs residential generators: 28–33% LHV; 30% is the midpoint.
const DEFAULT_ETA_ELECTRIC: f64 = 0.30;

/// Thermal recovery is disabled by default; set > 0.0 to activate CHP.
const DEFAULT_ETA_THERMAL: f64 = 0.0;

/// Engineering estimate: jacket water heat fraction at rated PLR.
/// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine: jacket fraction
/// is a manufacturer-supplied quadratic curve of PLR with no standard fixed value.
/// 0.30 is an engineering estimate consistent with typical IC engine heat rejection
/// at rated load; no primary-source measurement available for this field split.
const JACKET_FRACTION_OF_THERMAL: f64 = 0.30;

/// Engineering estimate: lube oil heat fraction at rated PLR.
/// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine: lube oil fraction
/// is a manufacturer-supplied quadratic curve of PLR with no standard fixed value.
/// 0.10 is an engineering estimate consistent with typical IC engine heat rejection
/// at rated load; no primary-source measurement available for this field split.
const LUBE_FRACTION_OF_THERMAL: f64 = 0.10;

/// Engineering estimate: exhaust heat fraction at rated PLR.
/// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine: exhaust fraction
/// is a manufacturer-supplied quadratic curve of PLR with no standard fixed value.
/// 0.60 is an engineering estimate consistent with typical IC engine heat rejection
/// at rated load; no primary-source measurement available for this field split.
const EXHAUST_FRACTION_OF_THERMAL: f64 = 0.60;

/// Conservative ramp rate for a residential-class reciprocating generator.
/// A 10 kW unit ramps to full load in ~10 s → 1 kW/s.
/// OCHRE uses 0.1 kW/min (~0.0017 kW/s) which is unrealistically slow for
/// residential reciprocating units; we use kW/s for finer-grained control.
const DEFAULT_DELTA_KW_PER_S: f64 = 1.0;

/// Default rated output power. 10 kW covers most North American whole-home loads.
const DEFAULT_RATED_POWER_KW: f64 = 10.0;

/// Default grid import limit for self-consumption control.
/// 0.0 means the generator covers 100 % of load; set to a positive value to
/// allow some baseline import and reduce generator cycling.
const DEFAULT_GRID_IMPORT_LIMIT_KW: f64 = 0.0;

/// Default export limit: no grid export is allowed.
/// Set > 0.0 to permit controlled islanding export.
const DEFAULT_EXPORT_LIMIT_KW: f64 = 0.0;

/// Default CHP fluid loop flow rate (kg/s).
/// 0.1 kg/s ≈ 6 L/min, typical for a small jacket-water heat recovery loop.
const DEFAULT_FLOW_RATE_KG_S: f64 = 0.1;

/// Default CHP supply temperature (°C) at rated generator output.
const DEFAULT_SUPPLY_TEMP_C: f64 = 70.0;

/// Default CHP return temperature (°C) entering the heat exchanger.
const DEFAULT_RETURN_TEMP_C: f64 = 60.0;

/// Engineering estimate: typical IC engine jacket water supply temperature (°C).
/// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine: jacket water
/// temperature emerges from PLR-dependent curves and the heat recovery loop model;
/// no standard fixed value exists. 90°C is a typical residential IC engine value.
const DEFAULT_SUPPLY_TEMP_JACKET_C: f64 = 90.0;

/// Engineering estimate: typical IC engine exhaust temperature at HX inlet (°C).
/// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine: exhaust temperature
/// is modelled via PLR-dependent exhaust gas temperature curves and NTU-effectiveness
/// HX; no standard fixed value exists. 450°C is a midpoint in the typical 400–500°C
/// range for IC engines.
const DEFAULT_SUPPLY_TEMP_EXHAUST_C: f64 = 450.0;

/// Default inverter efficiency for fuel cells: 95 % DC-to-AC.
/// Residential fuel cell inverters typically range 93–97 %.
/// EnergyPlus FuelCellElectricGenerator.cc:2112-2113 — constant inverter model.
const DEFAULT_INVERTER_EFFICIENCY: f64 = 0.95;

/// Default stack operating temperature for a residential PEMFC (°C).
/// PEM fuel cells operate at 60–80 °C; SOFCs run much hotter (600–1000 °C)
/// but residential units are typically PEM-based.
const DEFAULT_STACK_TEMP_C: f64 = 70.0;

/// Default stack cooler polynomial coefficient r0.
/// At nominal temperature, ~20 % of DC power is rejected as stack heat.
const DEFAULT_STACK_COOLER_R0: f64 = 0.20;

/// Default stack cooler polynomial coefficient r1 (1/°C).
/// At default 0.0, there is no temperature offset sensitivity.
const DEFAULT_STACK_COOLER_R1: f64 = 0.0;

/// Default stack cooler polynomial coefficient r2 (1/W).
const DEFAULT_STACK_COOLER_R2: f64 = 0.0;

/// Default stack cooler polynomial coefficient r3 (1/W²).
const DEFAULT_STACK_COOLER_R3: f64 = 0.0;

/// Default stack cooler nominal temperature (°C).
/// Matches the default operating temperature.
const DEFAULT_STACK_NOMINAL_TEMP_C: f64 = 70.0;

const IDLE_KW_THRESHOLD: f64 = 1e-6;

/// Compute stack cooler heat removal using the EnergyPlus polynomial.
///
/// EnergyPlus FuelCellElectricGenerator.cc:1859-1865:
///   qs_cool = (r0 + r1*(Tstack - Tnom)) * (1 + r2*Pel + r3*Pel²) * Pel
///
/// All inputs must be in SI: Pel (W), temperatures (°C). r0 dimensionless,
/// r1 (1/°C), r2 (1/W), r3 (1/W²) — matching EnergyPlus source convention.
/// Returns stack cooler heat in W.
fn compute_stack_cooler_heat(
    pel_w: f64,
    tstack_c: f64,
    tnom_c: f64,
    r0: f64,
    r1: f64,
    r2: f64,
    r3: f64,
) -> f64 {
    let dt = tstack_c - tnom_c;
    let temp_factor = r0 + r1 * dt;
    let power_factor = 1.0 + r2 * pel_w + r3 * pel_w * pel_w;
    (temp_factor * power_factor * pel_w).max(0.0)
}

/// Resolve the per-stream heat recovery efficiencies from config.
///
/// When per-stream fields (`eta_jacket_water` / `eta_lube_oil` / `eta_exhaust`) are
/// present, use them directly. When only legacy `eta_thermal` is provided, distribute it
/// using engineering-estimate split fractions: jacket ~30 %, lube ~10 %, exhaust ~60 %.
///
/// EnergyPlus uses PLR-dependent curves for per-stream heat recovery, not fixed
/// fractions (ICEngineElectricGenerator.cc:596–649).
///
/// Returns `(eta_jacket_water, eta_lube_oil, eta_exhaust)`.
fn resolve_heat_recovery_etas(config: &Option<&GeneratorConfig>) -> (f64, f64, f64) {
    let cfg = match config {
        Some(c) => c,
        None => return (0.0, 0.0, 0.0),
    };

    // When any per-stream field is explicitly set, use per-stream values.
    let any_per_stream =
        cfg.eta_jacket_water.is_some() || cfg.eta_lube_oil.is_some() || cfg.eta_exhaust.is_some();
    if any_per_stream {
        return (
            cfg.eta_jacket_water.unwrap_or(0.0),
            cfg.eta_lube_oil.unwrap_or(0.0),
            cfg.eta_exhaust.unwrap_or(0.0),
        );
    }

    // Legacy path: distribute eta_thermal using engineering-estimate split fractions.
    let eta_thermal = cfg.eta_thermal.unwrap_or(DEFAULT_ETA_THERMAL);
    if eta_thermal > 0.0 {
        (
            eta_thermal * JACKET_FRACTION_OF_THERMAL,
            eta_thermal * LUBE_FRACTION_OF_THERMAL,
            eta_thermal * EXHAUST_FRACTION_OF_THERMAL,
        )
    } else {
        (0.0, 0.0, 0.0)
    }
}

// ---------------------------------------------------------------------------
// EfficiencyModel -- separated concern for computing load-dependent efficiency
// ---------------------------------------------------------------------------

/// Load-dependent electrical efficiency model.
///
/// Matches OCHRE's three efficiency types:
///   - `Constant`: fixed efficiency at all loads (default for gas generators)
///   - `Curve`: piecewise-linear interpolation from (capacity_ratio, efficiency_ratio) pairs
///   - `Quadratic`: analytical curve from Vishwanathan et al. (2018), default for fuel cells
///
/// OCHRE reference: `Generator.calculate_efficiency()`, lines 148-171.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EfficiencyModel {
    /// Fixed efficiency at all output levels.
    Constant { rated: f64 },
    /// Piecewise-linear curve: efficiency = rated * interp(capacity_ratio → efficiency_ratio).
    /// Points must be sorted by capacity_ratio ascending, first point at 0.0.
    Curve { rated: f64, points: Vec<(f64, f64)> },
    /// Quadratic curve from Vishwanathan et al. (2018):
    ///   eff = rated * (-0.5 * cr² + 1.5 * cr)
    /// where cr = |P_electric| / capacity.
    /// Reference: Appl Energy, https://doi.org/10.1016/j.apenergy.2018.06.013
    ///
    /// Note: OCHRE has a bug at line 169: `return min(eff, 0.001)` should be
    /// `max(eff, 0.001)`. We use `max` here for correct clamping.
    Quadratic { rated: f64 },
}

impl EfficiencyModel {
    /// Evaluate efficiency at a given capacity ratio (0.0 to 1.0).
    /// Returns 0.0 when capacity_ratio is zero (equipment off).
    pub fn evaluate(&self, capacity_ratio: f64) -> f64 {
        if capacity_ratio.abs() < f64::EPSILON {
            return 0.0;
        }
        let cr = capacity_ratio.clamp(0.0, 1.0);
        match self {
            Self::Constant { rated } => *rated,
            Self::Curve { rated, points } => {
                let eff_ratio = Self::interpolate_curve(points, cr);
                rated * eff_ratio
            }
            Self::Quadratic { rated } => {
                // Vishwanathan (2018): eff = rated * (-0.5 * cr^2 + 1.5 * cr)
                let eff = rated * (-0.5 * cr * cr + 1.5 * cr);
                // Floor matches OCHRE Generator.py line 169 (fixing the min/max typo there).
                eff.max(0.001) // must be positive
            }
        }
    }

    /// Piecewise-linear interpolation on sorted (x, y) points.
    fn interpolate_curve(points: &[(f64, f64)], x: f64) -> f64 {
        if points.is_empty() {
            return 1.0;
        }
        if points.len() == 1 {
            return points[0].1;
        }
        if x <= points[0].0 {
            return points[0].1;
        }
        let last = points.len() - 1;
        if x >= points[last].0 {
            return points[last].1;
        }
        for i in 0..last {
            let (x0, y0) = points[i];
            let (x1, y1) = points[i + 1];
            if x <= x1 {
                let span = x1 - x0;
                if span.abs() < f64::EPSILON {
                    return y0;
                }
                let t = (x - x0) / span;
                return y0 + t * (y1 - y0);
            }
        }
        points[last].1
    }

    fn rated(&self) -> f64 {
        match self {
            Self::Constant { rated } | Self::Curve { rated, .. } | Self::Quadratic { rated } => {
                *rated
            }
        }
    }

    /// Validate the model parameters.
    fn validate(&self) -> Result<(), HaresError> {
        let rated = self.rated();
        if !rated.is_finite() || rated <= 0.0 || rated > 1.0 {
            return Err(HaresError::Equipment(
                "generator eta_electric must be in (0, 1]".to_string(),
            ));
        }
        if let Self::Curve { points, .. } = self {
            Self::validate_curve_pairs(points)?;
        }
        Ok(())
    }

    fn validate_curve_points(points: &[GeneratorEfficiencyCurvePoint]) -> Result<(), HaresError> {
        if points.len() < 2 {
            return Err(HaresError::Equipment(
                "generator efficiency curve requires at least 2 points".to_string(),
            ));
        }
        for point in points {
            if !point.capacity_ratio.is_finite() || !(0.0..=1.0).contains(&point.capacity_ratio) {
                return Err(HaresError::Equipment(
                    "generator efficiency curve capacity_ratio must be finite and within [0, 1]"
                        .to_string(),
                ));
            }
            if !point.efficiency_ratio.is_finite() || point.efficiency_ratio < 0.0 {
                return Err(HaresError::Equipment(
                    "generator efficiency curve efficiency_ratio must be finite and >= 0"
                        .to_string(),
                ));
            }
        }
        for window in points.windows(2) {
            if window[1].capacity_ratio <= window[0].capacity_ratio {
                return Err(HaresError::Equipment(
                    "generator efficiency curve points must be strictly increasing by capacity_ratio"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }

    fn validate_curve_pairs(points: &[(f64, f64)]) -> Result<(), HaresError> {
        let typed_points = points
            .iter()
            .map(
                |(capacity_ratio, efficiency_ratio)| GeneratorEfficiencyCurvePoint {
                    capacity_ratio: *capacity_ratio,
                    efficiency_ratio: *efficiency_ratio,
                },
            )
            .collect::<Vec<_>>();
        Self::validate_curve_points(&typed_points)
    }

    fn default_curve_points() -> Vec<(f64, f64)> {
        // OCHRE default curve: (0,0), (0.5,1), (1,1)
        vec![(0.0, 0.0), (0.5, 1.0), (1.0, 1.0)]
    }

    fn curve_pairs(points: &[GeneratorEfficiencyCurvePoint]) -> Vec<(f64, f64)> {
        points
            .iter()
            .map(|point| (point.capacity_ratio, point.efficiency_ratio))
            .collect()
    }

    /// Build from the typed generator config.
    fn from_typed_config(
        config: &GeneratorConfig,
        kind: GeneratorKind,
    ) -> Result<Self, HaresError> {
        let rated = config.eta_electric.unwrap_or(DEFAULT_ETA_ELECTRIC);
        let eff_type = config
            .efficiency_type
            .as_deref()
            .unwrap_or(kind.default_efficiency_type());
        match eff_type {
            "curve" => {
                let points = config
                    .efficiency_curve_points
                    .as_deref()
                    .map(Self::curve_pairs)
                    .unwrap_or_else(Self::default_curve_points);
                Ok(Self::Curve { rated, points })
            }
            "quadratic" => Ok(Self::Quadratic { rated }),
            "constant" => Ok(Self::Constant { rated }),
            _ => Err(HaresError::Equipment(format!(
                "unrecognised generator efficiency_type '{eff_type}'; \
                 expected one of: constant, curve, quadratic"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// GeneratorKind
// ---------------------------------------------------------------------------

/// Variant tag — used for labelling, default efficiency type, and fuel-cell-specific physics branching.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeneratorKind {
    GasGenerator,
    FuelCell,
}

impl GeneratorKind {
    fn equipment_type(self) -> &'static str {
        match self {
            Self::GasGenerator => "Gas Generator",
            Self::FuelCell => "Gas Fuel Cell",
        }
    }

    /// Default efficiency type per OCHRE convention.
    /// Gas generators use constant; fuel cells use curve.
    fn default_efficiency_type(self) -> &'static str {
        match self {
            Self::GasGenerator => "constant",
            Self::FuelCell => "curve",
        }
    }
}

// ---------------------------------------------------------------------------
// Checkpoint
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct GeneratorCheckpoint {
    current_power_kw: f64,
    mode: OperatingMode,
    power_setpoint_kw: Option<f64>,
    self_consumption_enabled: bool,
}

// ---------------------------------------------------------------------------
// Generator struct
// ---------------------------------------------------------------------------

pub struct Generator {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    kind: GeneratorKind,

    // Static config
    rated_power_kw: f64,
    /// Minimum operating power for self-consumption mode (kW).
    /// Generator will not run below this level; it shuts off instead.
    /// OCHRE: `capacity_min` parameter.
    capacity_min_kw: Option<f64>,
    efficiency: EfficiencyModel,
    /// Thermal recovery efficiency; 0.0 means no CHP.
    eta_thermal: f64,
    /// Per-stream heat recovery fractions.
    /// EnergyPlus ICEngineElectricGenerator.cc:596–649.
    eta_jacket_water: f64,
    eta_lube_oil: f64,
    eta_exhaust: f64,
    /// Maximum output-power change per second (kW/s).
    delta_kw_per_s: f64,
    grid_import_limit_kw: f64,
    export_limit_kw: f64,

    // CHP fluid port (active when eta_thermal > 0.0 and loop_id is configured)
    chp_loop_id: Option<LoopId>,
    flow_rate_kg_s: f64,
    supply_temp_c: f64,
    return_temp_c: f64,
    /// Jacket water supply temperature (°C) for telemetry. ~90°C typical.
    supply_temp_jacket_c: f64,
    /// Exhaust heat exchanger supply temperature (°C) for telemetry. ~400–500°C typical.
    supply_temp_exhaust_c: f64,

    // Fuel-cell-specific physics (ignored for combustion generators)
    /// DC-to-AC inverter efficiency [0, 1]. 1.0 for combustion generators
    /// (no inverter). Default 0.95 for fuel cells.
    inverter_efficiency: f64,
    /// Stack operating temperature (°C).
    stack_temp_c: f64,
    /// Stack cooler polynomial coefficients r0..r3.
    /// EnergyPlus FuelCellElectricGenerator.cc:1862:
    ///   qs_cool = (r0 + r1*(Tstack - Tnom)) * (1 + r2*Pel + r3*Pel²) * Pel
    stack_cooler_r0: f64,
    stack_cooler_r1: f64,
    stack_cooler_r2: f64,
    stack_cooler_r3: f64,
    stack_nominal_temp_c: f64,

    /// Maximum heat recovery fluid temperature (°C). None = no capping.
    heat_rec_max_temp_c: Option<f64>,

    // Dynamic state
    current_power_kw: f64,
    mode: OperatingMode,
    power_setpoint_kw: Option<f64>,
    /// When false, generator is locked off regardless of net load.
    /// Set via `SelfConsumption { enabled: false }`.
    self_consumption_enabled: bool,
}

impl Generator {
    #[must_use]
    pub fn new(config: EquipmentConfig, kind: GeneratorKind) -> Self {
        let typed = config.typed::<GeneratorConfig>().ok();
        let equipment_id = typed.as_ref().and_then(|c| c.equipment_id).unwrap_or(0);
        let zone = typed.as_ref().and_then(|c| c.zone_id).map(ZoneId);
        let fuel = typed
            .as_ref()
            .and_then(|c| c.fuel_type)
            .unwrap_or(FuelType::Gas);
        let eta_thermal = typed
            .as_ref()
            .and_then(|c| c.eta_thermal)
            .unwrap_or(DEFAULT_ETA_THERMAL);
        let (eta_jacket_water, eta_lube_oil, eta_exhaust) =
            resolve_heat_recovery_etas(&typed.as_ref());
        let has_thermal = eta_jacket_water > 0.0 || eta_lube_oil > 0.0 || eta_exhaust > 0.0;
        let loop_raw = typed.as_ref().and_then(|c| c.loop_id).map(LoopId);
        let chp_loop_id = if has_thermal {
            loop_raw.filter(|lid| lid.0 != 0)
        } else {
            None
        };

        let mut ports = vec![PortDeclaration::electrical(), PortDeclaration::fuel()];
        if let Some(z) = zone {
            ports.push(PortDeclaration::thermal(z));
        }
        if let Some(lid) = chp_loop_id {
            ports.push(PortDeclaration::fluid(lid, FluidType::Water));
        }

        let has_chp = has_thermal;
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id),
            name: config.name.clone(),
            end_use: EndUse::GENERATOR,
            equipment_type: Cow::Borrowed(kind.equipment_type()),
            zone,
            fuel,
            stage: ExecutionStage::Electrical,
            control_capabilities: ControlCapabilities::POWER_SETPOINT
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::SELF_CONSUMPTION,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::FUEL
                | CoreCapabilities::HAS_MODE,
            telemetry_fields: generator_telemetry_fields(has_chp, kind == GeneratorKind::FuelCell),
            zone_type: None,
        };

        let efficiency = typed.as_ref().map_or_else(
            || EfficiencyModel::Constant {
                rated: DEFAULT_ETA_ELECTRIC,
            },
            |c| match EfficiencyModel::from_typed_config(c, kind) {
                Ok(model) => model,
                Err(e) => {
                    tracing::warn!("{e}: falling back to constant efficiency");
                    EfficiencyModel::Constant {
                        rated: DEFAULT_ETA_ELECTRIC,
                    }
                }
            },
        );

        Self {
            descriptor,
            ports,
            telemetry: default_telemetry(has_chp, kind == GeneratorKind::FuelCell),
            core_output: CoreOutput::default(),
            kind,
            rated_power_kw: typed
                .as_ref()
                .map(|c| c.rated_power_kw)
                .unwrap_or(DEFAULT_RATED_POWER_KW),
            capacity_min_kw: typed.as_ref().and_then(|c| c.capacity_min_kw),
            efficiency,
            eta_thermal,
            eta_jacket_water,
            eta_lube_oil,
            eta_exhaust,
            delta_kw_per_s: typed
                .as_ref()
                .and_then(|c| c.delta_kw_per_s)
                .unwrap_or(DEFAULT_DELTA_KW_PER_S),
            grid_import_limit_kw: typed
                .as_ref()
                .and_then(|c| c.grid_import_limit_kw)
                .unwrap_or(DEFAULT_GRID_IMPORT_LIMIT_KW),
            export_limit_kw: typed
                .as_ref()
                .and_then(|c| c.export_limit_kw)
                .unwrap_or(DEFAULT_EXPORT_LIMIT_KW),
            chp_loop_id,
            flow_rate_kg_s: typed
                .as_ref()
                .and_then(|c| c.flow_rate_kg_s)
                .unwrap_or(DEFAULT_FLOW_RATE_KG_S),
            supply_temp_c: typed
                .as_ref()
                .and_then(|c| c.supply_temp_c)
                .unwrap_or(DEFAULT_SUPPLY_TEMP_C),
            return_temp_c: typed
                .as_ref()
                .and_then(|c| c.return_temp_c)
                .unwrap_or(DEFAULT_RETURN_TEMP_C),
            supply_temp_jacket_c: DEFAULT_SUPPLY_TEMP_JACKET_C,
            supply_temp_exhaust_c: DEFAULT_SUPPLY_TEMP_EXHAUST_C,
            inverter_efficiency: typed
                .as_ref()
                .and_then(|c| c.inverter_efficiency)
                .unwrap_or(if kind == GeneratorKind::FuelCell {
                    DEFAULT_INVERTER_EFFICIENCY
                } else {
                    1.0
                }),
            stack_temp_c: typed
                .as_ref()
                .and_then(|c| c.stack_temp_c)
                .unwrap_or(DEFAULT_STACK_TEMP_C),
            stack_cooler_r0: typed
                .as_ref()
                .and_then(|c| c.stack_cooler_r0)
                .unwrap_or(DEFAULT_STACK_COOLER_R0),
            stack_cooler_r1: typed
                .as_ref()
                .and_then(|c| c.stack_cooler_r1)
                .unwrap_or(DEFAULT_STACK_COOLER_R1),
            stack_cooler_r2: typed
                .as_ref()
                .and_then(|c| c.stack_cooler_r2)
                .unwrap_or(DEFAULT_STACK_COOLER_R2),
            stack_cooler_r3: typed
                .as_ref()
                .and_then(|c| c.stack_cooler_r3)
                .unwrap_or(DEFAULT_STACK_COOLER_R3),
            stack_nominal_temp_c: typed
                .as_ref()
                .and_then(|c| c.stack_nominal_temp_c)
                .unwrap_or(DEFAULT_STACK_NOMINAL_TEMP_C),
            heat_rec_max_temp_c: typed.as_ref().and_then(|c| c.heat_rec_max_temp_c),
            current_power_kw: 0.0,
            mode: OperatingMode::Off,
            power_setpoint_kw: None,
            self_consumption_enabled: true,
        }
    }

    /// Clamp `target_kw` to enforce the ramp-rate limit on power increases.
    ///
    /// OCHRE Generator.py:129 -- ramp rate only constrains increasing generation.
    /// Shutdown and power reduction are instantaneous.
    fn apply_ramp_limit(&self, target_kw: f64, dt_s: f64) -> f64 {
        let delta = target_kw - self.current_power_kw;
        let clamped_delta = if delta > 0.0 {
            delta.min(self.delta_kw_per_s * dt_s)
        } else {
            delta // no ramp limit on decrease
        };
        (self.current_power_kw + clamped_delta).clamp(0.0, self.max_ac_kw())
    }

    /// Maximum grid-visible AC output (kW), accounting for inverter losses for fuel cells.
    fn max_ac_kw(&self) -> f64 {
        if self.kind == GeneratorKind::FuelCell {
            self.rated_power_kw * self.inverter_efficiency
        } else {
            self.rated_power_kw
        }
    }

    /// Determine the unconstrained target power before ramp-rate limiting.
    ///
    /// Priority 1: explicit power setpoint.
    /// Priority 2: self-consumption controller (OCHRE `update_internal_control` equivalent).
    ///
    /// Self-consumption formula (matches OCHRE lines 108-116):
    ///   desired_import = clamp(net_load, -export_limit, import_limit)
    ///   target_generation = net_load - desired_import
    ///
    /// When `capacity_min_kw` is set, generator will not operate below that level;
    /// it shuts off instead (OCHRE `get_power_limits` min operating power).
    /// This check is applied after resolving the target regardless of source.
    fn determine_target_kw(&self, net_load_kw: f64) -> f64 {
        let max_ac = self.max_ac_kw();
        let raw = if let Some(sp) = self.power_setpoint_kw {
            sp.clamp(0.0, max_ac)
        } else if !self.self_consumption_enabled {
            // SelfConsumption disabled: generator is locked off unless an
            // explicit PowerSetpoint is active.
            0.0
        } else {
            // Self-consumption: match OCHRE formula exactly.
            // desired_import = clamp(net_load, -export_limit, import_limit)
            // target = net_load - desired_import
            let desired_import = net_load_kw
                .min(self.grid_import_limit_kw)
                .max(-self.export_limit_kw);
            (net_load_kw - desired_import).clamp(0.0, max_ac)
        };

        // Enforce minimum operating power regardless of control source.
        // OCHRE Generator.py get_power_limits line 136-138: max_power = -capacity_min,
        // which in OCHRE's negative-generation convention means the generator must
        // produce at least capacity_min when it is on. Values between 0 and capacity_min
        // are clamped UP to capacity_min; a request of exactly 0 keeps the generator off.
        if let Some(min_kw) = self.capacity_min_kw {
            if raw > 0.0 && raw < min_kw {
                return min_kw;
            }
        }
        raw
    }

    fn init_typed(&mut self, config: &EquipmentConfig) -> crate::Result<()> {
        let c = config.require_typed::<GeneratorConfig>("Generator")?;
        c.validate()?;

        if let Some(fuel_type) = c.fuel_type {
            self.descriptor.fuel = fuel_type;
        }
        self.descriptor.id = EquipmentId(c.equipment_id.unwrap_or(self.descriptor.id.0));
        self.descriptor.zone = c.zone_id.map(ZoneId);
        self.rated_power_kw = c.rated_power_kw;
        self.capacity_min_kw = c.capacity_min_kw;
        self.eta_thermal = c.eta_thermal.unwrap_or(self.eta_thermal);
        let (eta_jacket_water, eta_lube_oil, eta_exhaust) = resolve_heat_recovery_etas(&Some(&c));
        self.eta_jacket_water = eta_jacket_water;
        self.eta_lube_oil = eta_lube_oil;
        self.eta_exhaust = eta_exhaust;
        self.delta_kw_per_s = c.delta_kw_per_s.unwrap_or(self.delta_kw_per_s);
        self.grid_import_limit_kw = c.grid_import_limit_kw.unwrap_or(self.grid_import_limit_kw);
        self.export_limit_kw = c.export_limit_kw.unwrap_or(self.export_limit_kw);
        self.flow_rate_kg_s = c.flow_rate_kg_s.unwrap_or(self.flow_rate_kg_s);
        self.supply_temp_c = c.supply_temp_c.unwrap_or(self.supply_temp_c);
        self.return_temp_c = c.return_temp_c.unwrap_or(self.return_temp_c);
        self.inverter_efficiency =
            c.inverter_efficiency
                .unwrap_or(if self.kind == GeneratorKind::FuelCell {
                    DEFAULT_INVERTER_EFFICIENCY
                } else {
                    1.0
                });
        self.stack_temp_c = c.stack_temp_c.unwrap_or(self.stack_temp_c);
        self.stack_cooler_r0 = c.stack_cooler_r0.unwrap_or(self.stack_cooler_r0);
        self.stack_cooler_r1 = c.stack_cooler_r1.unwrap_or(self.stack_cooler_r1);
        self.stack_cooler_r2 = c.stack_cooler_r2.unwrap_or(self.stack_cooler_r2);
        self.stack_cooler_r3 = c.stack_cooler_r3.unwrap_or(self.stack_cooler_r3);
        self.stack_nominal_temp_c = c.stack_nominal_temp_c.unwrap_or(self.stack_nominal_temp_c);
        self.heat_rec_max_temp_c = c.heat_rec_max_temp_c;

        self.efficiency = EfficiencyModel::from_typed_config(&c, self.kind)?;
        self.efficiency.validate()?;

        if self.eta_jacket_water > 0.0 || self.eta_lube_oil > 0.0 || self.eta_exhaust > 0.0 {
            if let Some(lid) = c.loop_id {
                if lid == 0 {
                    return Err(HaresError::Equipment(
                        "generator CHP fluid loop_id must be non-zero".to_string(),
                    ));
                }
                self.chp_loop_id = Some(LoopId(lid));
            }
        }
        let mut ports = vec![PortDeclaration::electrical(), PortDeclaration::fuel()];
        if let Some(zone) = self.descriptor.zone {
            ports.push(PortDeclaration::thermal(zone));
        }
        if let Some(loop_id) = self.chp_loop_id {
            ports.push(PortDeclaration::fluid(loop_id, FluidType::Water));
        }
        self.ports = ports;

        self.current_power_kw = 0.0;
        self.mode = OperatingMode::Off;
        self.power_setpoint_kw = None;
        self.self_consumption_enabled = true;

        let has_chp =
            self.eta_jacket_water > 0.0 || self.eta_lube_oil > 0.0 || self.eta_exhaust > 0.0;
        self.descriptor.telemetry_fields =
            generator_telemetry_fields(has_chp, self.kind == GeneratorKind::FuelCell);
        self.telemetry = default_telemetry(has_chp, self.kind == GeneratorKind::FuelCell);
        self.telemetry
            .set(tk::ETA_ELECTRIC, self.efficiency.rated());
        self.core_output = CoreOutput::default();

        Ok(())
    }
}

impl Equipment for Generator {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        self.init_typed(config)
    }

    fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
        // Derive a provisional mode from the pending setpoint or current output
        // so callers see an up-to-date value before the next step() runs.
        // OperatingMode has no dedicated "On" variant; Standby is used for active
        // generation (matches OCHRE's generator state machine conventions).
        let target = self.power_setpoint_kw.unwrap_or(self.current_power_kw);
        if target > IDLE_KW_THRESHOLD {
            self.mode = OperatingMode::Standby;
        } else {
            self.mode = OperatingMode::Off;
        }
        self.mode
    }

    fn step(
        &mut self,
        _env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let dt_s = dt.as_secs_f64();
        if dt_s <= 0.0 {
            return Err(HaresError::Equipment(
                "timestep must be positive".to_string(),
            ));
        }

        // Read Stage 1 accumulated net load (load + any earlier generation).
        let net_load_kw = power_w_to_kw(ports.electrical.net_active_w());

        let unconstrained_kw = self.determine_target_kw(net_load_kw);
        // Ramp rate only constrains power increases (OCHRE Generator.py:129).
        // Flag is true only when ramping up and the increase exceeds the limit.
        let ramp_delta = unconstrained_kw - self.current_power_kw;
        let ramp_limited =
            ramp_delta > 0.0 && ramp_delta > self.delta_kw_per_s * dt_s + IDLE_KW_THRESHOLD;
        let output_kw = self.apply_ramp_limit(unconstrained_kw, dt_s);

        self.current_power_kw = output_kw;

        // Fuel cells use a DC intermediate with inverter conversion.
        // Combustion generators send output_kw directly to the grid.
        // For fuel cells, output_kw is the AC target; the stack must produce
        // more DC to compensate for inverter losses.
        let is_fuel_cell = self.kind == GeneratorKind::FuelCell;
        let dc_kw = if is_fuel_cell && output_kw > IDLE_KW_THRESHOLD {
            output_kw / self.inverter_efficiency
        } else {
            output_kw
        };
        let dc_power_w = power_kw_to_w(dc_kw);

        // Capacity ratio uses DC power for fuel cells (stack rating),
        // AC power for combustion generators.
        // EnergyPlus FuelCellElectricGenerator.cc:1697 — curve vs Pel/NomPel.
        let capacity_ratio = dc_kw / self.rated_power_kw;
        let eta = self.efficiency.evaluate(capacity_ratio);

        // Derive fuel and heat flows.
        // Fuel is computed from DC stack power, not AC output.
        let fuel_w = if output_kw > IDLE_KW_THRESHOLD && eta > 0.0 {
            dc_power_w / eta
        } else {
            0.0
        };
        let electrical_w = power_kw_to_w(output_kw);
        // EnergyPlus FuelCellElectricGenerator.cc:2112-2116 — inverter model.
        let inverter_loss_w = if is_fuel_cell {
            dc_power_w - electrical_w
        } else {
            0.0
        };
        // EnergyPlus FuelCellElectricGenerator.cc:1859-1865 — stack cooler polynomial.
        let stack_cooling_w = if is_fuel_cell && output_kw > IDLE_KW_THRESHOLD {
            compute_stack_cooler_heat(
                dc_power_w,
                self.stack_temp_c,
                self.stack_nominal_temp_c,
                self.stack_cooler_r0,
                self.stack_cooler_r1,
                self.stack_cooler_r2,
                self.stack_cooler_r3,
            )
        } else {
            0.0
        };
        // Per-stream heat recovery: jacket water, lube oil, exhaust.
        // EnergyPlus ICEngineElectricGenerator.cc:596–649 — multi-stream heat recovery.
        // Each stream has a different recoverable fraction and temperature quality.
        let q_jacket_w = fuel_w * self.eta_jacket_water;
        let q_lube_w = fuel_w * self.eta_lube_oil;
        let q_exhaust_w = fuel_w * self.eta_exhaust;
        let q_thermal_available_w = q_jacket_w + q_lube_w + q_exhaust_w;
        // Total waste heat = fuel - AC electrical; includes inverter loss and stack heat.
        let total_waste_w = fuel_w - electrical_w;
        let q_flue_w = total_waste_w - q_thermal_available_w;

        let has_thermal =
            self.eta_jacket_water > 0.0 || self.eta_lube_oil > 0.0 || self.eta_exhaust > 0.0;

        // Heat recovery temperature capping (EnergyPlus HRecRatio).
        // EnergyPlus ICEngineElectricGenerator.cc:729-793 — CalcICEngineGenHeatRecovery.
        // When the fluid loop cannot absorb all available heat without exceeding the
        // maximum temperature setpoint, thermal output is scaled down by HRecRatio.
        // Rejected heat is routed to q_flue (waste heat to zone / ambient).
        let (
            heat_rec_ratio,
            q_thermal_effective_w,
            q_jacket_eff_w,
            q_lube_eff_w,
            q_exhaust_eff_w,
            q_flue_eff_w,
        ) = if has_thermal && q_thermal_available_w > IDLE_KW_THRESHOLD {
            if let Some(max_temp) = self.heat_rec_max_temp_c {
                let cp = CP_LIQUID_WATER_J_KG_K;
                let loop_return_c = self.return_temp_c;
                let temp_rise = q_thermal_available_w / (self.flow_rate_kg_s * cp);
                if (loop_return_c + temp_rise) > max_temp {
                    // Max heat the loop can absorb without exceeding HeatRecMaxTemp.
                    // EnergyPlus ICEngineElectricGenerator.cc:771:
                    //   MinHeatRecMdot = EnergyRecovered / (Cp * (HeatRecMaxTemp - HeatRecInTemp))
                    // HRecRatio = HeatRecMdot / MinHeatRecMdot
                    //   = (flow_rate * Cp * (Tmax - Treturn)) / q_thermal_available
                    let q_max_absorbable = self.flow_rate_kg_s * cp * (max_temp - loop_return_c);
                    let ratio = (q_max_absorbable / q_thermal_available_w).clamp(0.0, 1.0);
                    let q_rejected = q_thermal_available_w - q_thermal_available_w * ratio;
                    (
                        ratio,
                        q_thermal_available_w * ratio,
                        q_jacket_w * ratio,
                        q_lube_w * ratio,
                        q_exhaust_w * ratio,
                        q_flue_w + q_rejected,
                    )
                } else {
                    (
                        1.0,
                        q_thermal_available_w,
                        q_jacket_w,
                        q_lube_w,
                        q_exhaust_w,
                        q_flue_w,
                    )
                }
            } else {
                (
                    1.0,
                    q_thermal_available_w,
                    q_jacket_w,
                    q_lube_w,
                    q_exhaust_w,
                    q_flue_w,
                )
            }
        } else {
            (0.0, 0.0, 0.0, 0.0, 0.0, q_flue_w)
        };

        // Update supply temperature from actual thermal output so the fluid port
        // carries self-consistent data: flow × Cp × ΔT = declared thermal_power_w
        // by construction. Without this, the generator would write a dynamically
        // computed thermal_power_w alongside static config temperatures, causing
        // the fluid solver invariant to fire on mismatched flow-implied energy.
        // EnergyPlus ICEngineElectricGenerator.cc:763:
        //   HeatRecOutTemp = EnergyRecovered / (HeatRecMdot × CpHeatRec) + HeatRecInTemp
        if has_thermal && q_thermal_effective_w > IDLE_KW_THRESHOLD && self.flow_rate_kg_s > 0.0 {
            self.supply_temp_c = self.return_temp_c
                + q_thermal_effective_w / (self.flow_rate_kg_s * CP_LIQUID_WATER_J_KG_K);
        }

        // Invariant: heat_rec_ratio must be in [0, 1] and effective <= available.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            debug_assert!(
                (0.0..=1.0).contains(&heat_rec_ratio),
                "heat_rec_ratio must be in [0, 1], got {heat_rec_ratio}"
            );
            debug_assert!(
                q_thermal_effective_w
                    <= q_thermal_available_w + 10.0 * f64::EPSILON * q_thermal_available_w.abs(),
                "q_thermal_effective ({q_thermal_effective_w} W) must not exceed q_thermal_available ({q_thermal_available_w} W)"
            );
        }

        // Invariant: energy conservation within the generator.
        // q_jacket + q_lube + q_exhaust must not exceed fuel_w - electrical_w.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let margin = 10.0 * f64::EPSILON * fuel_w.abs().max(1.0);
            debug_assert!(
                q_thermal_available_w <= total_waste_w + margin,
                "generator per-stream heat recovery ({q_thermal_available_w} W) exceeds available waste heat ({total_waste_w} W)"
            );
        }

        // Invariant: when CHP is active with a fluid port, the thermal power
        // declared to the fluid port (thermal_power_w) must equal the generator's
        // computed effective thermal output. A gap means energy was computed but
        // never deposited into any accumulator — a silent energy routing bug.
        // This invariant guards the fix in T-0084: adding thermal_power_w to
        // PortContribution::Fluid so the generator can quantitatively transfer
        // energy to the loop model.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if has_thermal
                && self.chp_loop_id.is_some()
                && q_thermal_effective_w > IDLE_KW_THRESHOLD
            {
                // q_thermal_effective_w is the value that will be written to the fluid port
                // as PortContribution::Fluid.thermal_power_w. This assertion confirms we
                // are not accidentally writing zero or a wrong value — it catches the class
                // of bug where energy is computed (telemetry reports it) but never reaches
                // any accumulator.
                debug_assert!(
                    q_thermal_effective_w > 0.0,
                    "generator CHP active with fluid port but q_thermal_effective_w is \
                     non-positive ({q_thermal_effective_w} W); energy routing gap"
                );
            }
        }

        // Observer capture: record heat recovery capping diagnostics.
        #[cfg(feature = "observe")]
        {
            if has_thermal && q_thermal_available_w > IDLE_KW_THRESHOLD {
                let loop_return_c = self.return_temp_c;
                tracing::debug!(
                    q_thermal_available_w,
                    q_thermal_effective_w,
                    heat_rec_ratio,
                    loop_return_temp_c = loop_return_c,
                    q_jacket_water_w = q_jacket_eff_w,
                    q_lube_oil_w = q_lube_eff_w,
                    q_exhaust_water_w = q_exhaust_eff_w,
                    "Generator heat recovery capping",
                );
            }
        }

        // Energy routing to zone and fluid ports:
        //   - CHP with fluid port: q_thermal_effective → fluid, q_flue_effective → zone (no double-count)
        //   - CHP without fluid port: all non-electrical loss → zone
        //   - No CHP: all non-electrical loss → zone (q_flue only, since q_thermal=0)
        //
        // When routing to zone without a fluid port, thermal categories are
        // differentiated by temperature grade:
        //   - Jacket water + lube oil (~85–90°C): JacketLoss (low-grade, for DHW preheat)
        //   - Exhaust (~400–500°C): InternalGain (high-grade, usable for absorption chilling)
        //   - Flue loss (residual): InternalGain
        //   EnergyPlus ICEngineElectricGenerator.cc:596–649 — multi-stream heat recovery.
        let (zone_jacket_loss_w, zone_internal_gain_w) =
            if has_thermal && self.chp_loop_id.is_some() {
                // Fluid port takes all q_thermal_effective; zone only gets flue loss.
                (0.0, q_flue_eff_w)
            } else {
                // No fluid port: zone receives all waste heat, split by category.
                // Jacket + lube → JacketLoss; exhaust + flue → InternalGain.
                if has_thermal {
                    (
                        q_jacket_eff_w + q_lube_eff_w,
                        q_exhaust_eff_w + q_flue_eff_w,
                    )
                } else {
                    // No heat recovery at all: all waste heat → InternalGain.
                    (0.0, total_waste_w)
                }
            };

        // Write electrical port (negative = generation).
        ports.accumulate(&PortContribution::Electrical {
            active_power_w: power_kw_to_w(-output_kw),
            reactive_power_kvar: 0.0,
        })?;

        // Write fuel port.
        if fuel_w > 0.0 {
            ports.accumulate(&PortContribution::Fuel {
                fuel_type: FuelType::Gas,
                consumption_w: fuel_w,
            })?;
        }

        // Write zone thermal ports, differentiated by category.
        if zone_jacket_loss_w > IDLE_KW_THRESHOLD {
            if let Some(zone) = self.descriptor.zone {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: zone_jacket_loss_w,
                    radiant_gain_w: 0.0,
                    latent_gain_w: 0.0,
                    category: ThermalCategory::JacketLoss,
                })?;
            }
        }
        if zone_internal_gain_w > IDLE_KW_THRESHOLD {
            if let Some(zone) = self.descriptor.zone {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: zone_internal_gain_w,
                    radiant_gain_w: 0.0,
                    latent_gain_w: 0.0,
                    category: ThermalCategory::InternalGain,
                })?;
            }
        }

        // Write CHP fluid port when producing heat.
        if q_thermal_effective_w > IDLE_KW_THRESHOLD {
            if let Some(loop_id) = self.chp_loop_id {
                ports.accumulate(&PortContribution::Fluid {
                    loop_id,
                    flow_rate_kg_s: self.flow_rate_kg_s,
                    supply_temp_c: self.supply_temp_c,
                    return_temp_c: self.return_temp_c,
                    fluid_type: FluidType::Water,
                    thermal_power_w: Some(q_thermal_effective_w),
                })?;
            }
        }

        self.mode = if output_kw > IDLE_KW_THRESHOLD {
            OperatingMode::Standby
        } else {
            OperatingMode::Off
        };

        self.telemetry.set(tk::ELECTRIC_OUTPUT_KW, output_kw);
        self.telemetry.set(tk::FUEL_INPUT_W, fuel_w);
        self.telemetry.set(tk::ETA_ELECTRIC, eta);
        self.telemetry
            .set(tk::RAMP_LIMITED, if ramp_limited { 1.0 } else { 0.0 });

        if has_thermal {
            self.telemetry
                .set(tk::THERMAL_AVAILABLE_W, q_thermal_available_w);
            self.telemetry
                .set(tk::THERMAL_OUTPUT_W, q_thermal_effective_w);
            // thermal_power_delivered_w tracks what was actually written to fluid port
            // contributions. When a fluid port exists, it equals q_thermal_effective_w
            // (set at the PortContribution::Fluid construction site). When CHP is active
            // without a fluid port, thermal power goes to zone ports instead, so
            // delivered_w stays 0.0 to reflect no fluid-port delivery.
            let delivered_w = if self.chp_loop_id.is_some() {
                q_thermal_effective_w
            } else {
                0.0
            };
            self.telemetry
                .set(tk::THERMAL_POWER_DELIVERED_W, delivered_w);
            self.telemetry.set(tk::HEAT_REC_RATIO, heat_rec_ratio);
            self.telemetry
                .set(tk::LOOP_RETURN_TEMP_C, self.return_temp_c);
            self.telemetry.set(tk::FLUE_LOSS_W, q_flue_eff_w);
            self.telemetry.set(tk::JACKET_WATER_W, q_jacket_eff_w);
            self.telemetry.set(tk::LUBE_OIL_W, q_lube_eff_w);
            self.telemetry.set(tk::EXHAUST_WATER_W, q_exhaust_eff_w);
            self.telemetry
                .set(tk::SUPPLY_TEMP_JACKET_C, self.supply_temp_jacket_c);
            self.telemetry
                .set(tk::SUPPLY_TEMP_EXHAUST_C, self.supply_temp_exhaust_c);

            // Observer capture: record per-stream heat recovery for diagnostics.
            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    q_jacket_water_w = q_jacket_eff_w,
                    q_lube_oil_w = q_lube_eff_w,
                    q_exhaust_water_w = q_exhaust_eff_w,
                    supply_temp_jacket_c = self.supply_temp_jacket_c,
                    supply_temp_exhaust_c = self.supply_temp_exhaust_c,
                    eta_jacket_water = self.eta_jacket_water,
                    eta_lube_oil = self.eta_lube_oil,
                    eta_exhaust = self.eta_exhaust,
                    "Generator step complete — per-stream heat recovery",
                );
            }
        }
        if is_fuel_cell {
            self.telemetry.set(tk::FUEL_CELL_DC_KW, dc_kw);
            self.telemetry
                .set(tk::FUEL_CELL_INVERTER_LOSS_W, inverter_loss_w);
            self.telemetry
                .set(tk::FUEL_CELL_STACK_HEAT_W, stack_cooling_w);
            // Observer capture: record fuel cell internal physics for diagnostics.
            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    fuel_cell_dc_kw = dc_kw,
                    fuel_cell_inverter_loss_w = inverter_loss_w,
                    fuel_cell_stack_heat_w = stack_cooling_w,
                    inverter_efficiency = self.inverter_efficiency,
                    stack_temp_c = self.stack_temp_c,
                    "FuelCell step complete",
                );
            }
        }
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Generation(output_kw.max(0.0))),
                reactive_power_kvar: None,
                fuel_w: Some(FuelPower {
                    fuel_type: FuelType::Gas,
                    consumption_w: fuel_w.max(0.0),
                }),
                thermal_output_w: if has_thermal && q_thermal_effective_w > 0.0 {
                    Some(q_thermal_effective_w)
                } else {
                    None
                },
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.mode),
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
            &GeneratorCheckpoint {
                current_power_kw: self.current_power_kw,
                mode: self.mode,
                power_setpoint_kw: self.power_setpoint_kw,
                self_consumption_enabled: self.self_consumption_enabled,
            },
            Self::checkpoint_version(),
            "Generator",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let cp: GeneratorCheckpoint = load_versioned(
            state,
            Self::checkpoint_version(),
            "Generator",
            self.descriptor().id,
        )?;
        self.current_power_kw = cp.current_power_kw;
        self.mode = cp.mode;
        self.power_setpoint_kw = cp.power_setpoint_kw;
        self.self_consumption_enabled = cp.self_consumption_enabled;

        // Recompute derived telemetry from restored power so consumers see
        // consistent values without needing to run a step first.
        let is_fuel_cell = self.kind == GeneratorKind::FuelCell;
        let dc_kw = if is_fuel_cell && self.current_power_kw > IDLE_KW_THRESHOLD {
            self.current_power_kw / self.inverter_efficiency
        } else {
            self.current_power_kw
        };
        let capacity_ratio = dc_kw / self.rated_power_kw;
        let eta = self.efficiency.evaluate(capacity_ratio);
        let fuel_w = if self.current_power_kw > IDLE_KW_THRESHOLD && eta > 0.0 {
            power_kw_to_w(dc_kw) / eta
        } else {
            0.0
        };

        self.telemetry
            .set(tk::ELECTRIC_OUTPUT_KW, self.current_power_kw);
        self.telemetry.set(tk::FUEL_INPUT_W, fuel_w);
        self.telemetry.set(tk::ETA_ELECTRIC, eta);

        let has_thermal =
            self.eta_jacket_water > 0.0 || self.eta_lube_oil > 0.0 || self.eta_exhaust > 0.0;
        if has_thermal {
            let q_jacket_w = fuel_w * self.eta_jacket_water;
            let q_lube_w = fuel_w * self.eta_lube_oil;
            let q_exhaust_w = fuel_w * self.eta_exhaust;
            let q_thermal_w = q_jacket_w + q_lube_w + q_exhaust_w;
            let q_flue_w = fuel_w - power_kw_to_w(self.current_power_kw) - q_thermal_w;
            self.telemetry.set(tk::THERMAL_AVAILABLE_W, q_thermal_w);
            self.telemetry.set(tk::THERMAL_OUTPUT_W, q_thermal_w);
            // On load_state, the checkpoint does not know whether capping was
            // active. Assume full delivery to fluid port when CHP is configured.
            self.telemetry.set(
                tk::THERMAL_POWER_DELIVERED_W,
                if self.chp_loop_id.is_some() {
                    q_thermal_w
                } else {
                    0.0
                },
            );
            self.telemetry.set(tk::HEAT_REC_RATIO, 1.0);
            self.telemetry
                .set(tk::LOOP_RETURN_TEMP_C, self.return_temp_c);
            self.telemetry.set(tk::FLUE_LOSS_W, q_flue_w);
            self.telemetry.set(tk::JACKET_WATER_W, q_jacket_w);
            self.telemetry.set(tk::LUBE_OIL_W, q_lube_w);
            self.telemetry.set(tk::EXHAUST_WATER_W, q_exhaust_w);
            self.telemetry
                .set(tk::SUPPLY_TEMP_JACKET_C, self.supply_temp_jacket_c);
            self.telemetry
                .set(tk::SUPPLY_TEMP_EXHAUST_C, self.supply_temp_exhaust_c);
        }
        if is_fuel_cell {
            let dc_power_w = power_kw_to_w(dc_kw);
            let inverter_loss_w = power_kw_to_w(dc_kw - self.current_power_kw);
            let stack_cooling_w = compute_stack_cooler_heat(
                dc_power_w,
                self.stack_temp_c,
                self.stack_nominal_temp_c,
                self.stack_cooler_r0,
                self.stack_cooler_r1,
                self.stack_cooler_r2,
                self.stack_cooler_r3,
            );
            self.telemetry.set(tk::FUEL_CELL_DC_KW, dc_kw);
            self.telemetry
                .set(tk::FUEL_CELL_INVERTER_LOSS_W, inverter_loss_w);
            self.telemetry
                .set(tk::FUEL_CELL_STACK_HEAT_W, stack_cooling_w);
        }
        let q_thermal_w = fuel_w * self.eta_thermal;
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Generation(self.current_power_kw.max(0.0))),
                reactive_power_kvar: None,
                fuel_w: Some(FuelPower {
                    fuel_type: FuelType::Gas,
                    consumption_w: fuel_w.max(0.0),
                }),
                thermal_output_w: if has_thermal && q_thermal_w > 0.0 && self.chp_loop_id.is_some()
                {
                    Some(q_thermal_w)
                } else {
                    None
                },
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.mode),
                soc: None,
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };

        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                if !active_power_kw.is_finite() {
                    return Err(HaresError::Control(
                        "generator PowerSetpoint active_power_kw must be finite".to_string(),
                    ));
                }
                // Setpoint persists until explicitly cleared (via ModeOverride::Standby
                // or a new PowerSetpoint). This matches OCHRE's behaviour where the
                // controller retains the last commanded value across timesteps, avoiding
                // oscillation when the caller does not re-send a setpoint every step.
                self.power_setpoint_kw = Some(active_power_kw.max(0.0));
            }
            ControlSignal::ModeOverride { mode } => match mode {
                OperatingMode::Off => {
                    self.power_setpoint_kw = Some(0.0);
                }
                OperatingMode::Standby => {
                    // Return to self-consumption control.
                    self.power_setpoint_kw = None;
                }
                other => {
                    return Err(HaresError::Control(format!(
                        "generator ModeOverride only supports Off and Standby, got {other:?}"
                    )));
                }
            },
            ControlSignal::SelfConsumption { enabled, .. } => {
                // Toggle self-consumption mode on or off.
                // When disabled, generator is locked off unless a PowerSetpoint is
                // active. Any existing setpoint is preserved so that re-enabling
                // does not leave a stale setpoint.
                self.self_consumption_enabled = *enabled;
            }
            _ => {
                return Err(HaresError::Control(format!(
                    "generator does not handle control signal: {signal:?}"
                )));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Wrapper types with delegation
// ---------------------------------------------------------------------------

pub struct GasGenerator {
    core: Generator,
}

pub struct FuelCell {
    core: Generator,
}

delegate_equipment!(GasGenerator, core);
delegate_equipment!(FuelCell, core);

// ---------------------------------------------------------------------------
// Registry registration
// ---------------------------------------------------------------------------

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Gas Generator",
        Box::new(|config| {
            Box::new(GasGenerator {
                core: Generator::new(config, GeneratorKind::GasGenerator),
            })
        }),
    );
    registry.register(
        "Gas Fuel Cell",
        Box::new(|config| {
            Box::new(FuelCell {
                core: Generator::new(config, GeneratorKind::FuelCell),
            })
        }),
    );
}

// ---------------------------------------------------------------------------
// Telemetry helpers
// ---------------------------------------------------------------------------

fn default_telemetry(has_chp: bool, is_fuel_cell: bool) -> Telemetry {
    // Base: 4 fields. CHP: 11 extra (thermal, available, delivered, ratio, loop_return,
    // flue, jacket, lube, exhaust, +2 supply temps).
    let capacity = if has_chp { 15 } else { 4 } + if is_fuel_cell { 3 } else { 0 };
    let mut t = Telemetry::with_capacity(capacity);
    t.insert(tk::ELECTRIC_OUTPUT_KW, 0.0);
    t.insert(tk::FUEL_INPUT_W, 0.0);
    t.insert(tk::ETA_ELECTRIC, 0.0);
    t.insert(tk::RAMP_LIMITED, 0.0);
    if has_chp {
        t.insert(tk::THERMAL_AVAILABLE_W, 0.0);
        t.insert(tk::THERMAL_OUTPUT_W, 0.0);
        t.insert(tk::THERMAL_POWER_DELIVERED_W, 0.0);
        t.insert(tk::HEAT_REC_RATIO, 0.0);
        t.insert(tk::LOOP_RETURN_TEMP_C, 0.0);
        t.insert(tk::FLUE_LOSS_W, 0.0);
        t.insert(tk::JACKET_WATER_W, 0.0);
        t.insert(tk::LUBE_OIL_W, 0.0);
        t.insert(tk::EXHAUST_WATER_W, 0.0);
        t.insert(tk::SUPPLY_TEMP_JACKET_C, 0.0);
        t.insert(tk::SUPPLY_TEMP_EXHAUST_C, 0.0);
    }
    if is_fuel_cell {
        t.insert(tk::FUEL_CELL_DC_KW, 0.0);
        t.insert(tk::FUEL_CELL_INVERTER_LOSS_W, 0.0);
        t.insert(tk::FUEL_CELL_STACK_HEAT_W, 0.0);
    }
    t
}

fn generator_telemetry_fields(has_chp: bool, is_fuel_cell: bool) -> Vec<TelemetryField> {
    let mut fields = vec![
        TelemetryField {
            name: tk::ELECTRIC_OUTPUT_KW.to_string(),
            unit: "kW".to_string(),
            description: "Electrical generation output".to_string(),
        },
        TelemetryField {
            name: tk::FUEL_INPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Fuel input power (P_electric / eta)".to_string(),
        },
        TelemetryField {
            name: tk::ETA_ELECTRIC.to_string(),
            unit: "-".to_string(),
            description: "Effective electrical efficiency at current load [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::RAMP_LIMITED.to_string(),
            unit: "-".to_string(),
            description: "1.0 when this step's power change was clamped by ramp-rate limit"
                .to_string(),
        },
    ];
    if has_chp {
        fields.push(TelemetryField {
            name: tk::THERMAL_AVAILABLE_W.to_string(),
            unit: "W".to_string(),
            description: "Uncapped CHP thermal recovery available before heat_rec_ratio scaling"
                .to_string(),
        });
        fields.push(TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description:
                "Total CHP thermal recovery output delivered to loop (after heat_rec_ratio)"
                    .to_string(),
        });
        fields.push(TelemetryField {
            name: tk::THERMAL_POWER_DELIVERED_W.to_string(),
            unit: "W".to_string(),
            description:
                "Sum of thermal_power_w written to fluid port contributions (cross-validates against THERMAL_OUTPUT_W)"
                    .to_string(),
        });
        fields.push(TelemetryField {
            name: tk::HEAT_REC_RATIO.to_string(),
            unit: "-".to_string(),
            description:
                "Heat recovery ratio: fraction of available thermal delivered to loop [0..1]"
                    .to_string(),
        });
        fields.push(TelemetryField {
            name: tk::LOOP_RETURN_TEMP_C.to_string(),
            unit: "°C".to_string(),
            description: "Fluid loop return temperature at time of heat recovery computation"
                .to_string(),
        });
        fields.push(TelemetryField {
            name: tk::FLUE_LOSS_W.to_string(),
            unit: "W".to_string(),
            description: "Residual flue loss (P_fuel - P_electric - Q_thermal)".to_string(),
        });
        fields.push(TelemetryField {
            name: tk::JACKET_WATER_W.to_string(),
            unit: "W".to_string(),
            description: "Jacket water heat recovery (P_fuel * eta_jacket_water)".to_string(),
        });
        fields.push(TelemetryField {
            name: tk::LUBE_OIL_W.to_string(),
            unit: "W".to_string(),
            description: "Lube oil heat recovery (P_fuel * eta_lube_oil)".to_string(),
        });
        fields.push(TelemetryField {
            name: tk::EXHAUST_WATER_W.to_string(),
            unit: "W".to_string(),
            description: "Exhaust heat recovery (P_fuel * eta_exhaust)".to_string(),
        });
        fields.push(TelemetryField {
            name: tk::SUPPLY_TEMP_JACKET_C.to_string(),
            unit: "°C".to_string(),
            description: "Jacket water supply temperature for recovery".to_string(),
        });
        fields.push(TelemetryField {
            name: tk::SUPPLY_TEMP_EXHAUST_C.to_string(),
            unit: "°C".to_string(),
            description: "Exhaust HX supply temperature for recovery".to_string(),
        });
    }
    if is_fuel_cell {
        fields.push(TelemetryField {
            name: tk::FUEL_CELL_DC_KW.to_string(),
            unit: "kW".to_string(),
            description: "DC stack electrical output before inverter".to_string(),
        });
        fields.push(TelemetryField {
            name: tk::FUEL_CELL_INVERTER_LOSS_W.to_string(),
            unit: "W".to_string(),
            description: "Power lost in DC-to-AC inverter conversion".to_string(),
        });
        fields.push(TelemetryField {
            name: tk::FUEL_CELL_STACK_HEAT_W.to_string(),
            unit: "W".to_string(),
            description: "Stack cooling heat removed by stack cooler".to_string(),
        });
    }
    fields
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, FluidAccumulator, FluidType, FuelType, GridState,
        OperatingMode, PortSlots, PortType, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
        telemetry_keys as tk,
    };

    use crate::config::ConfigValue;

    use super::*;
    use crate::{Equipment, EquipmentConfig};

    fn base_env() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                outdoor_wet_bulb_c: 7.0,
                outdoor_enthalpy_j_kg: 22_800.0,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 7.0,
                pressure_kpa: 101.325,
                ghi_w_m2: 400.0,
                dni_w_m2: 300.0,
                dhi_w_m2: 100.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                solar_irradiance: vec![],
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
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
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid UTC timestamp"),
            time_res: ChronoDuration::minutes(5),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn gen_config(overrides: &[(&str, ConfigValue)]) -> EquipmentConfig {
        let mut cfg = GeneratorConfig {
            equipment_id: None,
            zone_id: None,
            fuel_type: None,
            rated_power_kw: 10.0,
            eta_electric: Some(0.30),
            eta_thermal: None,
            eta_jacket_water: None,
            eta_lube_oil: None,
            eta_exhaust: None,
            efficiency_type: None,
            efficiency_curve_points: None,
            delta_kw_per_s: Some(1.0),
            capacity_min_kw: None,
            grid_import_limit_kw: None,
            export_limit_kw: None,
            loop_id: None,
            flow_rate_kg_s: None,
            supply_temp_c: None,
            return_temp_c: None,
            inverter_efficiency: None,
            stack_temp_c: None,
            stack_cooler_r0: None,
            stack_cooler_r1: None,
            stack_cooler_r2: None,
            stack_cooler_r3: None,
            stack_nominal_temp_c: None,
            heat_rec_max_temp_c: None,
        };
        for (k, v) in overrides {
            match (*k, v) {
                (KEY_RATED_POWER_KW, ConfigValue::Float(value)) => cfg.rated_power_kw = *value,
                (KEY_ETA_ELECTRIC, ConfigValue::Float(value)) => cfg.eta_electric = Some(*value),
                (KEY_ETA_THERMAL, ConfigValue::Float(value)) => cfg.eta_thermal = Some(*value),
                (KEY_ETA_JACKET_WATER, ConfigValue::Float(value)) => {
                    cfg.eta_jacket_water = Some(*value)
                }
                (KEY_ETA_LUBE_OIL, ConfigValue::Float(value)) => cfg.eta_lube_oil = Some(*value),
                (KEY_ETA_EXHAUST, ConfigValue::Float(value)) => cfg.eta_exhaust = Some(*value),
                (KEY_EFFICIENCY_TYPE, ConfigValue::Text(value)) => {
                    cfg.efficiency_type = Some(value.clone())
                }
                (KEY_DELTA_KW_PER_S, ConfigValue::Float(value)) => {
                    cfg.delta_kw_per_s = Some(*value)
                }
                (KEY_CAPACITY_MIN_KW, ConfigValue::Float(value)) => {
                    cfg.capacity_min_kw = Some(*value)
                }
                (KEY_GRID_IMPORT_LIMIT_KW, ConfigValue::Float(value)) => {
                    cfg.grid_import_limit_kw = Some(*value)
                }
                (KEY_EXPORT_LIMIT_KW, ConfigValue::Float(value)) => {
                    cfg.export_limit_kw = Some(*value)
                }
                (KEY_LOOP_ID, ConfigValue::Float(value)) => cfg.loop_id = Some(*value as u16),
                (KEY_FLOW_RATE_KG_S, ConfigValue::Float(value)) => {
                    cfg.flow_rate_kg_s = Some(*value)
                }
                (KEY_SUPPLY_TEMP_C, ConfigValue::Float(value)) => cfg.supply_temp_c = Some(*value),
                (KEY_RETURN_TEMP_C, ConfigValue::Float(value)) => cfg.return_temp_c = Some(*value),
                (KEY_INVERTER_EFFICIENCY, ConfigValue::Float(value)) => {
                    cfg.inverter_efficiency = Some(*value)
                }
                (KEY_STACK_TEMP_C, ConfigValue::Float(value)) => cfg.stack_temp_c = Some(*value),
                (KEY_STACK_COOLER_R0, ConfigValue::Float(value)) => {
                    cfg.stack_cooler_r0 = Some(*value)
                }
                (KEY_STACK_COOLER_R1, ConfigValue::Float(value)) => {
                    cfg.stack_cooler_r1 = Some(*value)
                }
                (KEY_STACK_COOLER_R2, ConfigValue::Float(value)) => {
                    cfg.stack_cooler_r2 = Some(*value)
                }
                (KEY_STACK_COOLER_R3, ConfigValue::Float(value)) => {
                    cfg.stack_cooler_r3 = Some(*value)
                }
                (KEY_STACK_NOMINAL_TEMP_C, ConfigValue::Float(value)) => {
                    cfg.stack_nominal_temp_c = Some(*value)
                }
                (KEY_HEAT_REC_MAX_TEMP_C, ConfigValue::Float(value)) => {
                    cfg.heat_rec_max_temp_c = Some(*value)
                }
                (KEY_ZONE_ID, ConfigValue::Float(value)) => cfg.zone_id = Some(*value as u16),
                (KEY_EQUIPMENT_ID, ConfigValue::Float(value)) => {
                    cfg.equipment_id = Some(*value as u32)
                }
                _ => panic!("unsupported generator test override key/value: {k}"),
            }
        }
        EquipmentConfig::from_typed(
            "Test Generator".to_string(),
            "Gas Generator".to_string(),
            cfg,
        )
    }

    fn ports_for(generator: &Generator) -> PortSlots {
        let mut slots = PortSlots::default();
        for pd in generator.ports() {
            match pd.port_type {
                PortType::Thermal => {
                    if let Some(z) = pd.zone {
                        slots.thermal.push(ThermalAccumulator::new(z));
                    }
                }
                PortType::Fluid => {
                    if let Some(lid) = pd.loop_id {
                        slots
                            .fluid
                            .push(FluidAccumulator::new(lid, FluidType::Water));
                    }
                }
                _ => {}
            }
        }
        slots
    }

    /// Helper: ramp to steady-state with a setpoint and fast ramp.
    fn ramp_to_steady_state(generator: &mut Generator, setpoint_kw: f64, env: &EnvironmentState) {
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: setpoint_kw,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(generator);
        // With 1 kW/s and 1s steps, need rated_power_kw steps + 1 margin.
        let steps = (generator.rated_power_kw as usize) + 1;
        for _ in 0..steps {
            slots.zero();
            generator
                .step(env, Duration::from_secs(1), &mut slots)
                .unwrap();
        }
    }

    // =======================================================================
    // EfficiencyModel unit tests
    // =======================================================================

    #[test]
    fn constant_efficiency_returns_rated_at_all_loads() {
        let model = EfficiencyModel::Constant { rated: 0.30 };
        assert!((model.evaluate(0.0) - 0.0).abs() < f64::EPSILON);
        assert!((model.evaluate(0.5) - 0.30).abs() < f64::EPSILON);
        assert!((model.evaluate(1.0) - 0.30).abs() < f64::EPSILON);
    }

    #[test]
    fn curve_efficiency_interpolates_ochre_default() {
        // OCHRE default curve: (0,0), (0.5,1), (1,1)
        let model = EfficiencyModel::Curve {
            rated: 0.95,
            points: vec![(0.0, 0.0), (0.5, 1.0), (1.0, 1.0)],
        };
        assert!((model.evaluate(0.0) - 0.0).abs() < f64::EPSILON);
        // cr=0.25 → interp between (0,0) and (0.5,1) → er=0.5 → eff=0.95*0.5=0.475
        assert!((model.evaluate(0.25) - 0.475).abs() < 1e-9);
        // cr=0.5 → er=1.0 → eff=0.95
        assert!((model.evaluate(0.5) - 0.95).abs() < 1e-9);
        // cr=0.75 → interp between (0.5,1) and (1,1) → er=1.0 → eff=0.95
        assert!((model.evaluate(0.75) - 0.95).abs() < 1e-9);
        // cr=1.0 → er=1.0 → eff=0.95
        assert!((model.evaluate(1.0) - 0.95).abs() < 1e-9);
    }

    #[test]
    fn curve_efficiency_matches_ochre_fuel_cell_test() {
        // OCHRE test_generator.py line 139-148:
        //   eff at cr=1.0 (6 kW / 6 kW) → rated
        //   eff at cr=0.5 (3 kW / 6 kW) → rated (since curve is 1.0 from 0.5 onward)
        //   eff at cr=1/3 (2 kW / 6 kW) → rated * 2/3
        //   eff at cr=0.0 → 0.0
        let model = EfficiencyModel::Curve {
            rated: 0.95,
            points: vec![(0.0, 0.0), (0.5, 1.0), (1.0, 1.0)],
        };
        assert!((model.evaluate(1.0) - 0.95).abs() < 1e-9, "cr=1.0");
        assert!((model.evaluate(0.5) - 0.95).abs() < 1e-9, "cr=0.5");
        let expected = 0.95 * (2.0 / 3.0);
        assert!(
            (model.evaluate(1.0 / 3.0) - expected).abs() < 1e-9,
            "cr=1/3: got {}, expected {expected}",
            model.evaluate(1.0 / 3.0)
        );
        assert!((model.evaluate(0.0) - 0.0).abs() < f64::EPSILON, "cr=0");
    }

    #[test]
    fn quadratic_efficiency_matches_vishwanathan_formula() {
        // eff = rated * (-0.5 * cr^2 + 1.5 * cr)
        let model = EfficiencyModel::Quadratic { rated: 0.40 };
        // cr=1.0: eff = 0.40 * (-0.5 + 1.5) = 0.40 * 1.0 = 0.40
        assert!((model.evaluate(1.0) - 0.40).abs() < 1e-9);
        // cr=0.5: eff = 0.40 * (-0.125 + 0.75) = 0.40 * 0.625 = 0.25
        assert!((model.evaluate(0.5) - 0.25).abs() < 1e-9);
        // cr=0.0: returns 0 (off)
        assert!((model.evaluate(0.0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn efficiency_model_validate_rejects_invalid() {
        let m = EfficiencyModel::Constant { rated: 0.0 };
        assert!(m.validate().is_err());

        let m = EfficiencyModel::Curve {
            rated: 0.5,
            points: vec![(0.5, 1.0)], // only 1 point
        };
        assert!(m.validate().is_err());

        let m = EfficiencyModel::Curve {
            rated: 0.5,
            points: vec![(0.5, 1.0), (0.3, 0.8)], // unsorted
        };
        assert!(m.validate().is_err());
    }

    // =======================================================================
    // Descriptor / contract
    // =======================================================================

    #[test]
    fn descriptor_matches_ticket_contract() {
        let config = gen_config(&[]);
        let generator = Generator::new(config, GeneratorKind::GasGenerator);
        assert_eq!(generator.descriptor().end_use, EndUse::GENERATOR);
        assert_eq!(generator.descriptor().stage, ExecutionStage::Electrical);
        assert_eq!(generator.descriptor().fuel, FuelType::Gas);
        let caps = generator.descriptor().control_capabilities;
        assert!(caps.contains(ControlCapabilities::POWER_SETPOINT));
        assert!(caps.contains(ControlCapabilities::MODE_OVERRIDE));
        let names: Vec<&str> = generator
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        for expected in &[
            tk::ELECTRIC_OUTPUT_KW,
            tk::FUEL_INPUT_W,
            tk::ETA_ELECTRIC,
            tk::RAMP_LIMITED,
        ] {
            assert!(
                names.contains(expected),
                "missing telemetry field: {expected}"
            );
        }
    }

    #[test]
    fn fuel_cell_descriptor_has_distinct_equipment_type() {
        let fc = Generator::new(gen_config(&[]), GeneratorKind::FuelCell);
        assert_eq!(fc.descriptor().equipment_type, "Gas Fuel Cell");
    }

    // =======================================================================
    // Fuel consumption
    // =======================================================================

    #[test]
    fn fuel_consumption_at_rated_output_matches_formula() {
        let config = gen_config(&[]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        ramp_to_steady_state(&mut generator, 10.0, &base_env());

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let output_kw = generator.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap();
        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let expected_fuel_w = (output_kw / 0.30) * 1000.0;
        assert!(
            (fuel_w - expected_fuel_w).abs() < 1.0,
            "fuel_input_w={fuel_w}, expected={expected_fuel_w}"
        );
        // fuel accumulator is in W; should match directly
        assert!(
            (slots.fuel.get(FuelType::Gas) - fuel_w).abs() < 1.0,
            "fuel accumulator mismatch"
        );
    }

    #[test]
    fn curve_efficiency_changes_fuel_consumption_at_partial_load() {
        // With curve efficiency, partial load uses more fuel per kW than rated.
        let config = EquipmentConfig::from_typed(
            "Test Generator".to_string(),
            "Gas Generator".to_string(),
            GeneratorConfig {
                eta_electric: Some(0.95),
                efficiency_type: Some("curve".to_string()),
                efficiency_curve_points: Some(vec![
                    GeneratorEfficiencyCurvePoint {
                        capacity_ratio: 0.0,
                        efficiency_ratio: 0.0,
                    },
                    GeneratorEfficiencyCurvePoint {
                        capacity_ratio: 0.5,
                        efficiency_ratio: 1.0,
                    },
                    GeneratorEfficiencyCurvePoint {
                        capacity_ratio: 1.0,
                        efficiency_ratio: 1.0,
                    },
                ]),
                delta_kw_per_s: Some(100.0),
                ..minimal_generator_config()
            },
        );
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        // At rated (10 kW, cr=1.0): eta = 0.95*1.0 = 0.95
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        let eta_full = generator.telemetry().get(tk::ETA_ELECTRIC).unwrap();
        assert!(
            (eta_full - 0.95).abs() < 1e-6,
            "full load eta should be 0.95, got {eta_full}"
        );

        // At 25% load (2.5 kW, cr=0.25): eta = 0.95*0.5 = 0.475
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 2.5,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        slots.zero();
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        let eta_partial = generator.telemetry().get(tk::ETA_ELECTRIC).unwrap();
        assert!(
            (eta_partial - 0.475).abs() < 1e-6,
            "partial load eta should be 0.475, got {eta_partial}"
        );
        assert!(
            eta_partial < eta_full,
            "partial load should be less efficient"
        );
    }

    #[test]
    fn quadratic_efficiency_varies_with_load() {
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.40.into()),
            (
                KEY_EFFICIENCY_TYPE,
                ConfigValue::Text("quadratic".to_string()),
            ),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        // cr=1.0: eff = 0.40 * 1.0 = 0.40
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            (generator.telemetry().get(tk::ETA_ELECTRIC).unwrap() - 0.40).abs() < 1e-6,
            "full load"
        );

        // cr=0.5: eff = 0.40 * 0.625 = 0.25
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        slots.zero();
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            (generator.telemetry().get(tk::ETA_ELECTRIC).unwrap() - 0.25).abs() < 1e-6,
            "half load"
        );
    }

    // =======================================================================
    // Electrical port
    // =======================================================================

    #[test]
    fn electrical_port_is_negative_generation() {
        let config = gen_config(&[(KEY_DELTA_KW_PER_S, 100.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(slots.electrical.generation_power_w < 0.0);
        assert!(
            (slots.electrical.generation_power_w + generator.current_power_kw * 1000.0).abs() < 1.0
        );
    }

    // =======================================================================
    // Ramp rate
    // =======================================================================

    #[test]
    fn ramp_rate_limits_power_increase_per_step() {
        let config = gen_config(&[
            (KEY_DELTA_KW_PER_S, 2.0.into()),
            (KEY_RATED_POWER_KW, 10.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let dt = Duration::from_secs(1);
        let mut slots = ports_for(&generator);
        slots.zero();
        generator.step(&base_env(), dt, &mut slots).unwrap();
        assert!(
            (generator.current_power_kw - 2.0).abs() < 1e-9,
            "step 1: got {}",
            generator.current_power_kw
        );
        assert_eq!(generator.telemetry().get(tk::RAMP_LIMITED).unwrap(), 1.0);

        for _ in 0..4 {
            slots.zero();
            generator.step(&base_env(), dt, &mut slots).unwrap();
        }
        assert!(
            (generator.current_power_kw - 10.0).abs() < 1e-9,
            "step 5: got {}",
            generator.current_power_kw
        );
    }

    #[test]
    fn ramp_rate_does_not_limit_power_decrease() {
        // OCHRE Generator.py:129: ramp rate only constrains increasing generation.
        // Shutdown from 10 kW should happen in a single step, not be ramp-limited.
        let config = gen_config(&[(KEY_DELTA_KW_PER_S, 1.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        ramp_to_steady_state(&mut generator, 10.0, &base_env());

        assert!(
            (generator.current_power_kw - 10.0).abs() < 1e-9,
            "precondition: at 10 kW"
        );

        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 0.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            generator.current_power_kw < IDLE_KW_THRESHOLD,
            "shutdown from 10 kW should be instant (no ramp limit on decrease), got {}",
            generator.current_power_kw
        );
        assert_eq!(generator.mode, OperatingMode::Off);
    }

    // =======================================================================
    // Self-consumption controller
    // =======================================================================

    #[test]
    fn self_consumption_covers_net_load() {
        let config = gen_config(&[(KEY_DELTA_KW_PER_S, 100.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        let mut slots = ports_for(&generator);
        slots.electrical.load_power_w = 4000.0;
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            (generator.current_power_kw - 4.0).abs() < 1e-9,
            "got {}",
            generator.current_power_kw
        );
    }

    #[test]
    fn self_consumption_respects_import_limit() {
        let config = gen_config(&[
            (KEY_GRID_IMPORT_LIMIT_KW, 2.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        let mut slots = ports_for(&generator);
        slots.electrical.load_power_w = 5000.0;
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            (generator.current_power_kw - 3.0).abs() < 1e-9,
            "got {}",
            generator.current_power_kw
        );
    }

    #[test]
    fn self_consumption_does_not_export_beyond_limit() {
        let config = gen_config(&[(KEY_DELTA_KW_PER_S, 100.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(generator.current_power_kw < IDLE_KW_THRESHOLD);
        assert_eq!(generator.mode, OperatingMode::Off);
    }

    #[test]
    fn capacity_min_clamps_up_below_threshold() {
        // OCHRE Generator.py get_power_limits: capacity_min sets a floor on
        // operating power. Requests below capacity_min are clamped UP, not shut off.
        let config = gen_config(&[
            (KEY_CAPACITY_MIN_KW, 3.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        // Load of 2 kW is below capacity_min of 3 kW → clamp up to 3 kW.
        let mut slots = ports_for(&generator);
        slots.electrical.load_power_w = 2000.0;
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            (generator.current_power_kw - 3.0).abs() < 1e-9,
            "should clamp up to capacity_min: {}",
            generator.current_power_kw
        );

        // Load of 5 kW is above capacity_min → generator should run at requested load.
        slots.zero();
        slots.electrical.load_power_w = 5000.0;
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            (generator.current_power_kw - 5.0).abs() < 1e-9,
            "should run at 5 kW: {}",
            generator.current_power_kw
        );
    }

    #[test]
    fn capacity_min_clamps_setpoint_up() {
        // OCHRE Generator.py get_power_limits: setpoint below capacity_min
        // is clamped UP to capacity_min (not shut off).
        let config = gen_config(&[
            (KEY_CAPACITY_MIN_KW, 3.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        // Setpoint of 1 kW is below capacity_min of 3 kW → clamp up to 3 kW.
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            (generator.current_power_kw - 3.0).abs() < 1e-9,
            "setpoint below capacity_min should clamp up to 3 kW, got {}",
            generator.current_power_kw
        );

        // Setpoint of 5 kW is above capacity_min → generator runs at 5 kW.
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        slots.zero();
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            (generator.current_power_kw - 5.0).abs() < 1e-9,
            "setpoint above capacity_min should run at 5 kW, got {}",
            generator.current_power_kw
        );
    }

    #[test]
    fn setpoint_zero_turns_off_despite_capacity_min() {
        // A setpoint of exactly 0 must shut the generator off, not clamp up
        // to capacity_min_kw. Only positive-but-below-min values clamp up.
        let config = gen_config(&[
            (KEY_CAPACITY_MIN_KW, 3.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        // First bring generator online.
        ramp_to_steady_state(&mut generator, 5.0, &base_env());
        assert!(generator.current_power_kw > 0.0, "precondition: running");

        // Set power to 0 -- should shut off, not clamp to 3 kW.
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 0.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            generator.current_power_kw < IDLE_KW_THRESHOLD,
            "setpoint=0 should shut off generator, got {}",
            generator.current_power_kw
        );
        assert_eq!(generator.mode, OperatingMode::Off);
    }

    // =======================================================================
    // CHP thermal output
    // =======================================================================

    #[test]
    fn chp_thermal_output_matches_formula() {
        // CHP without fluid port: all non-electrical waste heat goes to zone.
        let config = gen_config(&[
            (KEY_ETA_THERMAL, 0.40.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 6.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let thermal_w = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let electric_kw = generator.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap();
        assert!(
            (thermal_w - fuel_w * 0.40).abs() < 1.0,
            "Q_thermal = P_fuel * eta_thermal"
        );
        // Without a fluid port, zone receives ALL waste heat (q_thermal + q_flue).
        let total_waste_w = fuel_w - electric_kw * 1000.0;
        assert!(
            (slots.thermal[0].sensible_gain_w - total_waste_w).abs() < 1.0,
            "zone should get all waste heat ({total_waste_w} W), got {} W",
            slots.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn chp_flue_loss_satisfies_energy_balance() {
        let config = gen_config(&[
            (KEY_ETA_THERMAL, 0.40.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        // fuel_input_w, thermal_output_w, flue_loss_w are in W; electric_output_kw is in kW
        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let electric_w = generator.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap() * 1000.0;
        let thermal_w = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let flue_w = generator.telemetry().get(tk::FLUE_LOSS_W).unwrap();
        assert!(
            (fuel_w - electric_w - thermal_w - flue_w).abs() < 1.0,
            "energy balance violated"
        );
    }

    // =======================================================================
    // Non-CHP waste heat
    // =======================================================================

    #[test]
    fn waste_heat_goes_to_zone_when_no_chp() {
        // Without CHP, all non-electrical energy becomes zone heat gains.
        // OCHRE: sensible_gain = (electric_kw - power_input) * 1000
        let config = gen_config(&[
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 6.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let electric_w = generator.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap() * 1000.0;
        let expected_heat_w = fuel_w - electric_w;

        assert!(expected_heat_w > 0.0, "waste heat must be positive");
        assert!(
            (slots.thermal[0].sensible_gain_w - expected_heat_w).abs() < 1.0,
            "zone heat should equal waste: got {}, expected {expected_heat_w}",
            slots.thermal[0].sensible_gain_w,
        );
    }

    // =======================================================================
    // CHP fluid port
    // =======================================================================

    #[test]
    fn chp_fluid_port_writes_when_loop_id_configured() {
        let config = gen_config(&[
            (KEY_ETA_THERMAL, 0.35.into()),
            (KEY_LOOP_ID, 7.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 8.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert_eq!(slots.fluid.len(), 1);
        assert!(slots.fluid[0].total_flow_kg_s > 0.0);
    }

    #[test]
    fn chp_fluid_loop_id_required_for_fluid_port() {
        let config = gen_config(&[(KEY_ETA_THERMAL, 0.35.into())]);
        let generator = Generator::new(config, GeneratorKind::GasGenerator);
        assert!(
            !generator
                .ports()
                .iter()
                .any(|p| p.port_type == PortType::Fluid),
            "no fluid port without loop_id"
        );
    }

    #[test]
    fn init_rejects_zero_loop_id_for_chp() {
        let config = gen_config(&[(KEY_ETA_THERMAL, 0.35.into()), (KEY_LOOP_ID, 0.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        assert!(
            generator
                .init(&config, &base_env())
                .unwrap_err()
                .to_string()
                .contains("loop_id")
        );
    }

    #[test]
    fn chp_with_fluid_port_zone_gets_flue_loss_not_q_thermal() {
        // When a CHP fluid port is configured, q_thermal goes to the fluid loop,
        // and the zone thermal port should receive only the flue loss (q_flue),
        // not q_thermal. Without this fix, q_thermal would be double-counted.
        let config = gen_config(&[
            (KEY_ETA_THERMAL, 0.40.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_LOOP_ID, 7.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 6.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let electric_w = generator.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap() * 1000.0;
        let thermal_w = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let flue_w = generator.telemetry().get(tk::FLUE_LOSS_W).unwrap();

        // Verify energy balance
        assert!(
            (fuel_w - electric_w - thermal_w - flue_w).abs() < 1.0,
            "energy balance violated"
        );

        // Zone should receive flue loss only (in W), not q_thermal
        assert!(
            (slots.thermal[0].sensible_gain_w - flue_w).abs() < 1.0,
            "zone should get q_flue ({flue_w} W), got {} W",
            slots.thermal[0].sensible_gain_w
        );

        // Fluid port should carry q_thermal as declared thermal_power_w and
        // the port data must be self-consistent (flow × Cp × ΔT ≈ thermal_power_w).
        assert_eq!(slots.fluid.len(), 1, "fluid port should be populated");
        assert!(
            thermal_w > 0.0,
            "q_thermal should be positive when CHP is active"
        );
        assert!(
            slots.fluid[0].total_flow_kg_s > 0.0,
            "fluid port should carry flow"
        );
        assert!(
            slots.fluid[0].total_thermal_power_w > 0.0,
            "fluid port should carry declared thermal_power_w"
        );
        let flow_implied_w = slots.fluid[0].total_flow_kg_s
            * CP_LIQUID_WATER_J_KG_K
            * (slots.fluid[0].mean_supply_temp_c - slots.fluid[0].mean_return_temp_c);
        let mismatch = (flow_implied_w - slots.fluid[0].total_thermal_power_w).abs();
        assert!(
            mismatch < 1.0,
            "fluid port data is self-inconsistent: flow-implied energy ({flow_implied_w} W) \
             != declared thermal_power_w ({declared} W); diff = {mismatch} W",
            declared = slots.fluid[0].total_thermal_power_w
        );
    }

    // =======================================================================
    // Per-stream heat recovery
    // =======================================================================

    #[test]
    fn per_stream_heat_recovery_sums_to_thermal_output() {
        // When per-stream eta fields are set directly, q_thermal_w must equal
        // the sum of the three individual streams.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.12.into()),
            (KEY_ETA_LUBE_OIL, 0.04.into()),
            (KEY_ETA_EXHAUST, 0.24.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 6.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let q_jacket_w = generator.telemetry().get(tk::JACKET_WATER_W).unwrap();
        let q_lube_w = generator.telemetry().get(tk::LUBE_OIL_W).unwrap();
        let q_exhaust_w = generator.telemetry().get(tk::EXHAUST_WATER_W).unwrap();
        let q_thermal_w = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();

        assert!(
            (q_jacket_w - fuel_w * 0.12).abs() < 1.0,
            "jacket water recovery should match eta_jacket_water"
        );
        assert!(
            (q_lube_w - fuel_w * 0.04).abs() < 1.0,
            "lube oil recovery should match eta_lube_oil"
        );
        assert!(
            (q_exhaust_w - fuel_w * 0.24).abs() < 1.0,
            "exhaust recovery should match eta_exhaust"
        );
        assert!(
            ((q_jacket_w + q_lube_w + q_exhaust_w) - q_thermal_w).abs() < 1.0,
            "per-stream sum must equal total thermal output"
        );
    }

    #[test]
    fn per_stream_fractions_applied_independently_at_partial_load() {
        // Each stream's eta is applied independently. At different PLRs,
        // each stream should scale proportionally with fuel_w.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.10.into()),
            (KEY_ETA_LUBE_OIL, 0.05.into()),
            (KEY_ETA_EXHAUST, 0.25.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        // Half load (5 kW): verify each stream scales independently.
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w_half = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let jacket_half = generator.telemetry().get(tk::JACKET_WATER_W).unwrap();
        let lube_half = generator.telemetry().get(tk::LUBE_OIL_W).unwrap();
        let exhaust_half = generator.telemetry().get(tk::EXHAUST_WATER_W).unwrap();

        assert!(
            (jacket_half - fuel_w_half * 0.10).abs() < 1.0,
            "jacket at half load"
        );
        assert!(
            (lube_half - fuel_w_half * 0.05).abs() < 1.0,
            "lube at half load"
        );
        assert!(
            (exhaust_half - fuel_w_half * 0.25).abs() < 1.0,
            "exhaust at half load"
        );

        // Full load (10 kW): total is higher but fractions remain the same.
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        slots.zero();
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w_full = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let jacket_full = generator.telemetry().get(tk::JACKET_WATER_W).unwrap();
        let lube_full = generator.telemetry().get(tk::LUBE_OIL_W).unwrap();
        let exhaust_full = generator.telemetry().get(tk::EXHAUST_WATER_W).unwrap();

        assert!(
            (jacket_full - fuel_w_full * 0.10).abs() < 1.0,
            "jacket at full load"
        );
        assert!(
            (lube_full - fuel_w_full * 0.05).abs() < 1.0,
            "lube at full load"
        );
        assert!(
            (exhaust_full - fuel_w_full * 0.25).abs() < 1.0,
            "exhaust at full load"
        );

        // At full load, each stream is larger than at half load.
        assert!(
            jacket_full > jacket_half,
            "jacket should increase with load"
        );
        assert!(lube_full > lube_half, "lube should increase with load");
        assert!(
            exhaust_full > exhaust_half,
            "exhaust should increase with load"
        );
    }

    #[test]
    fn legacy_eta_thermal_backward_compat_sums_to_thermal_output() {
        // When only eta_thermal is provided (no per-stream fields), the
        // backward-compat distribution (jacket 30%, lube 10%, exhaust 60%)
        // should sum to the original q_thermal_w = fuel_w * eta_thermal.
        let config = gen_config(&[
            (KEY_ETA_THERMAL, 0.40.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 6.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let q_jacket_w = generator.telemetry().get(tk::JACKET_WATER_W).unwrap();
        let q_lube_w = generator.telemetry().get(tk::LUBE_OIL_W).unwrap();
        let q_exhaust_w = generator.telemetry().get(tk::EXHAUST_WATER_W).unwrap();
        let q_thermal_w = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();

        // Legacy expectation: q_thermal_w = fuel_w * 0.40
        assert!(
            (q_thermal_w - fuel_w * 0.40).abs() < 1.0,
            "q_thermal should still match legacy formula"
        );
        // Per-stream sum should equal q_thermal_w
        let stream_sum = q_jacket_w + q_lube_w + q_exhaust_w;
        assert!(
            (stream_sum - q_thermal_w).abs() < 1.0,
            "per-stream sum should match total: sum={stream_sum}, q_thermal={q_thermal_w}"
        );
        // Engineering-estimate split fractions: jacket ~30%, lube ~10%, exhaust ~60%
        assert!(
            (q_jacket_w - q_thermal_w * 0.30).abs() < 1.0,
            "jacket should be ~30% of thermal: got {q_jacket_w}, expected ~{}",
            q_thermal_w * 0.30
        );
        assert!(
            (q_lube_w - q_thermal_w * 0.10).abs() < 1.0,
            "lube should be ~10% of thermal: got {q_lube_w}, expected ~{}",
            q_thermal_w * 0.10
        );
        assert!(
            (q_exhaust_w - q_thermal_w * 0.60).abs() < 1.0,
            "exhaust should be ~60% of thermal: got {q_exhaust_w}, expected ~{}",
            q_thermal_w * 0.60
        );
    }

    #[test]
    fn per_stream_energy_balance_closure() {
        // fuel_w == electrical_w + q_jacket_w + q_lube_w + q_exhaust_w + q_flue_w
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.12.into()),
            (KEY_ETA_LUBE_OIL, 0.04.into()),
            (KEY_ETA_EXHAUST, 0.24.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 7.5,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let electric_w = generator.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap() * 1000.0;
        let q_jacket_w = generator.telemetry().get(tk::JACKET_WATER_W).unwrap();
        let q_lube_w = generator.telemetry().get(tk::LUBE_OIL_W).unwrap();
        let q_exhaust_w = generator.telemetry().get(tk::EXHAUST_WATER_W).unwrap();
        let q_flue_w = generator.telemetry().get(tk::FLUE_LOSS_W).unwrap();

        let balance = fuel_w - electric_w - q_jacket_w - q_lube_w - q_exhaust_w - q_flue_w;
        assert!(
            balance.abs() < 1.0,
            "energy balance violation: fuel={fuel_w}, elec={electric_w}, jacket={q_jacket_w}, \
             lube={q_lube_w}, exhaust={q_exhaust_w}, flue={q_flue_w}, residual={balance}"
        );
    }

    #[test]
    fn per_stream_supply_temps_registered_as_telemetry() {
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.12.into()),
            (KEY_ETA_EXHAUST, 0.24.into()),
        ]);
        let generator = Generator::new(config, GeneratorKind::GasGenerator);
        let names: Vec<&str> = generator
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(
            names.contains(&tk::SUPPLY_TEMP_JACKET_C),
            "missing supply_temp_jacket_c"
        );
        assert!(
            names.contains(&tk::SUPPLY_TEMP_EXHAUST_C),
            "missing supply_temp_exhaust_c"
        );
    }

    #[test]
    fn per_stream_telemetry_values_are_set_when_chp_active() {
        // All per-stream telemetry keys must be non-zero when CHP is active.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.12.into()),
            (KEY_ETA_LUBE_OIL, 0.04.into()),
            (KEY_ETA_EXHAUST, 0.24.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 6.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let jacket_w = generator.telemetry().get(tk::JACKET_WATER_W).unwrap();
        let lube_w = generator.telemetry().get(tk::LUBE_OIL_W).unwrap();
        let exhaust_w = generator.telemetry().get(tk::EXHAUST_WATER_W).unwrap();
        let jacket_temp = generator.telemetry().get(tk::SUPPLY_TEMP_JACKET_C).unwrap();
        let exhaust_temp = generator
            .telemetry()
            .get(tk::SUPPLY_TEMP_EXHAUST_C)
            .unwrap();

        assert!(jacket_w > 0.0, "jacket water recovery should be positive");
        assert!(lube_w > 0.0, "lube oil recovery should be positive");
        assert!(exhaust_w > 0.0, "exhaust recovery should be positive");
        assert!(
            (jacket_temp - DEFAULT_SUPPLY_TEMP_JACKET_C).abs() < 1e-9,
            "supply_temp_jacket should be default"
        );
        assert!(
            (exhaust_temp - DEFAULT_SUPPLY_TEMP_EXHAUST_C).abs() < 1e-9,
            "supply_temp_exhaust should be default"
        );
    }

    #[test]
    fn per_stream_thermal_category_jacket_loss_when_no_fluid_port() {
        // Without a CHP fluid port, jacket+lube heat should be routed to
        // ThermalCategory::JacketLoss and exhaust+flue to InternalGain.
        // This tests the category-level splitting.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.10.into()),
            (KEY_ETA_LUBE_OIL, 0.05.into()),
            (KEY_ETA_EXHAUST, 0.25.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        // Total sensible gain = jacket + lube + exhaust + flue
        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let electric_w = generator.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap() * 1000.0;
        let total_waste_w = fuel_w - electric_w;

        assert!(
            slots.thermal.len() >= 1,
            "should have a thermal accumulator"
        );
        let acc = &slots.thermal[0];
        assert!(
            (acc.sensible_gain_w - total_waste_w).abs() < 1.0,
            "total zone sensible gain should equal waste heat: got {} W, expected {total_waste_w} W",
            acc.sensible_gain_w
        );

        // Jacket + lube should be in JacketLoss category
        let q_jacket_w = generator.telemetry().get(tk::JACKET_WATER_W).unwrap();
        let q_lube_w = generator.telemetry().get(tk::LUBE_OIL_W).unwrap();
        let jacket_loss_w = acc.sensible_for_category(ThermalCategory::JacketLoss);
        assert!(
            (jacket_loss_w - (q_jacket_w + q_lube_w)).abs() < 1.0,
            "JacketLoss category should contain jacket ({q_jacket_w}) + lube ({q_lube_w}), got {jacket_loss_w} W"
        );

        // Exhaust + flue should be in InternalGain category
        let q_exhaust_w = generator.telemetry().get(tk::EXHAUST_WATER_W).unwrap();
        let q_flue_w = generator.telemetry().get(tk::FLUE_LOSS_W).unwrap();
        let internal_gain_w = acc.sensible_for_category(ThermalCategory::InternalGain);
        assert!(
            (internal_gain_w - (q_exhaust_w + q_flue_w)).abs() < 1.0,
            "InternalGain category should contain exhaust ({q_exhaust_w}) + flue ({q_flue_w}), got {internal_gain_w} W"
        );
    }

    #[test]
    fn per_stream_load_state_restores_stream_telemetry() {
        // After save/load, the per-stream telemetry keys must be populated.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.15.into()),
            (KEY_ETA_LUBE_OIL, 0.05.into()),
            (KEY_ETA_EXHAUST, 0.20.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        ramp_to_steady_state(&mut generator, 8.0, &base_env());

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let bytes = generator.save_state().unwrap();
        let mut generator2 = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator2.init(&config, &base_env()).unwrap();
        generator2.load_state(&bytes).unwrap();

        assert!(
            generator2.telemetry().get(tk::JACKET_WATER_W).unwrap() > 0.0,
            "jacket water should be restored"
        );
        assert!(
            generator2.telemetry().get(tk::LUBE_OIL_W).unwrap() > 0.0,
            "lube oil should be restored"
        );
        assert!(
            generator2.telemetry().get(tk::EXHAUST_WATER_W).unwrap() > 0.0,
            "exhaust water should be restored"
        );
        assert!(
            generator2
                .telemetry()
                .get(tk::SUPPLY_TEMP_JACKET_C)
                .unwrap()
                > 0.0,
            "supply_temp_jacket should be restored"
        );
        assert!(
            generator2
                .telemetry()
                .get(tk::SUPPLY_TEMP_EXHAUST_C)
                .unwrap()
                > 0.0,
            "supply_temp_exhaust should be restored"
        );
    }

    #[test]
    fn per_stream_core_output_thermal_output_populated() {
        // When per-stream CHP is active, CoreOutput.flows.thermal_output_w
        // should be Some(q_thermal_w) rather than None.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.12.into()),
            (KEY_ETA_EXHAUST, 0.24.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let co = generator.core_output();
        assert!(
            co.flows.thermal_output_w.is_some(),
            "thermal_output_w should be populated when CHP is active"
        );
        assert!(
            co.flows.thermal_output_w.unwrap() > 0.0,
            "thermal_output_w should be positive"
        );
    }

    // =======================================================================
    // Save/load state
    // =======================================================================

    #[test]
    fn save_load_state_round_trip() {
        let config = gen_config(&[]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        ramp_to_steady_state(&mut generator, 10.0, &base_env());

        let saved = generator.current_power_kw;
        let bytes = generator.save_state().unwrap();

        let mut generator2 = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator2.init(&config, &base_env()).unwrap();
        generator2.load_state(&bytes).unwrap();

        assert!((generator2.current_power_kw - saved).abs() < f64::EPSILON);
        assert_eq!(generator2.power_setpoint_kw, Some(10.0));
        assert!(
            (generator2.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap() - saved).abs()
                < f64::EPSILON
        );
    }

    // =======================================================================
    // Efficiency validation
    // =======================================================================

    #[test]
    fn init_rejects_eta_sum_exceeding_one() {
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.60.into()),
            (KEY_ETA_THERMAL, 0.50.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        let err = generator.init(&config, &base_env()).unwrap_err();
        assert!(
            err.to_string()
                .contains("generator eta_electric + eta_thermal must not exceed 1.0"),
            "unexpected validation error: {err}"
        );
    }

    #[test]
    fn init_rejects_zero_eta_electric() {
        let config = gen_config(&[(KEY_ETA_ELECTRIC, 0.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        assert!(
            generator
                .init(&config, &base_env())
                .unwrap_err()
                .to_string()
                .contains("eta_electric")
        );
    }

    #[test]
    fn init_rejects_nonpositive_rated_power() {
        let config = gen_config(&[(KEY_RATED_POWER_KW, 0.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        assert!(
            generator
                .init(&config, &base_env())
                .unwrap_err()
                .to_string()
                .contains("rated_power_kw")
        );
    }

    #[test]
    fn init_sets_eta_electric_telemetry_to_configured_value() {
        let config = gen_config(&[(KEY_ETA_ELECTRIC, 0.45.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        assert!((generator.telemetry().get(tk::ETA_ELECTRIC).unwrap() - 0.45).abs() < f64::EPSILON);
    }

    #[test]
    fn init_rejects_invalid_capacity_min() {
        let config = gen_config(&[(KEY_CAPACITY_MIN_KW, 20.0.into())]); // > rated 10
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        assert!(generator.init(&config, &base_env()).is_err());
    }

    #[test]
    fn init_rejects_unrecognised_efficiency_type_with_named_value() {
        let config = gen_config(&[(
            KEY_EFFICIENCY_TYPE,
            "quadractic".into(), // typo: should be "quadratic"
        )]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        let err = generator
            .init(&config, &base_env())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("quadractic"),
            "error message must name the unrecognised value; got: {err}"
        );
        assert!(
            err.contains("efficiency_type"),
            "error message must mention the field name; got: {err}"
        );
    }

    // =======================================================================
    // Registry
    // =======================================================================

    #[test]
    fn both_registry_entries_exist() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("Gas Generator").is_some());
        assert!(registry.get("Gas Fuel Cell").is_some());
    }

    #[test]
    fn registry_instantiates_both_types() {
        let registry = EquipmentRegistry::new();
        let gg = registry.create("Gas Generator", gen_config(&[])).unwrap();
        assert_eq!(gg.descriptor().equipment_type, "Gas Generator");
        let fc = registry.create("Gas Fuel Cell", gen_config(&[])).unwrap();
        assert_eq!(fc.descriptor().equipment_type, "Gas Fuel Cell");
    }

    // =======================================================================
    // CHP telemetry presence
    // =======================================================================

    #[test]
    fn chp_telemetry_fields_present_when_eta_thermal_positive() {
        let generator = Generator::new(
            gen_config(&[(KEY_ETA_THERMAL, 0.30.into()), (KEY_ZONE_ID, 1.0.into())]),
            GeneratorKind::GasGenerator,
        );
        let names: Vec<&str> = generator
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(names.contains(&tk::THERMAL_OUTPUT_W));
        assert!(names.contains(&tk::FLUE_LOSS_W));
    }

    #[test]
    fn no_chp_telemetry_fields_without_eta_thermal() {
        let generator = Generator::new(gen_config(&[]), GeneratorKind::GasGenerator);
        let names: Vec<&str> = generator
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(!names.contains(&tk::THERMAL_OUTPUT_W));
        assert!(!names.contains(&tk::FLUE_LOSS_W));
    }

    // =======================================================================
    // Control signals
    // =======================================================================

    #[test]
    fn mode_override_off_stops_generation() {
        let config = gen_config(&[(KEY_DELTA_KW_PER_S, 100.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(generator.current_power_kw > 0.0);

        generator
            .apply_control(&ControlSignal::ModeOverride {
                mode: OperatingMode::Off,
            })
            .unwrap();
        slots.zero();
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(generator.current_power_kw < IDLE_KW_THRESHOLD);
    }

    #[test]
    fn mode_override_standby_restores_self_consumption() {
        let config = gen_config(&[(KEY_DELTA_KW_PER_S, 100.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        assert!(generator.power_setpoint_kw.is_some());

        generator
            .apply_control(&ControlSignal::ModeOverride {
                mode: OperatingMode::Standby,
            })
            .unwrap();
        assert!(generator.power_setpoint_kw.is_none());
    }

    #[test]
    fn self_consumption_capability_declared() {
        let generator = Generator::new(gen_config(&[]), GeneratorKind::GasGenerator);
        assert!(
            generator
                .descriptor()
                .control_capabilities
                .contains(ControlCapabilities::SELF_CONSUMPTION)
        );
    }

    #[test]
    fn self_consumption_signal_disables_generation() {
        let config = gen_config(&[(KEY_DELTA_KW_PER_S, 100.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        // Disable self-consumption mode.
        generator
            .apply_control(&ControlSignal::SelfConsumption {
                enabled: false,
                solar_only_charging: false,
            })
            .unwrap();

        // With net load present, generator should stay off because SC is disabled.
        let mut slots = ports_for(&generator);
        slots.electrical.load_power_w = 8.0;
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            generator.current_power_kw < IDLE_KW_THRESHOLD,
            "generator should be off when self_consumption disabled, got {}",
            generator.current_power_kw
        );

        // Re-enable self-consumption -- should start generating again.
        generator
            .apply_control(&ControlSignal::SelfConsumption {
                enabled: true,
                solar_only_charging: false,
            })
            .unwrap();
        slots.zero();
        slots.electrical.load_power_w = 8.0;
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            generator.current_power_kw > IDLE_KW_THRESHOLD,
            "generator should run after re-enabling self_consumption, got {}",
            generator.current_power_kw
        );
    }

    #[test]
    fn apply_control_rejects_unsupported_signal() {
        let mut generator = Generator::new(gen_config(&[]), GeneratorKind::GasGenerator);
        generator.init(&gen_config(&[]), &base_env()).unwrap();
        assert!(
            generator
                .apply_control(&ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: Some(20.0),
                    cooling_setpoint_c: None,
                    deadband_c: None,
                })
                .is_err()
        );
    }

    // =======================================================================
    // Off state
    // =======================================================================

    #[test]
    fn off_state_produces_no_output() {
        let config = gen_config(&[]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        assert!(generator.telemetry().get(tk::FUEL_INPUT_W).unwrap() < IDLE_KW_THRESHOLD);
        assert!(generator.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap() < IDLE_KW_THRESHOLD);
        assert!(slots.electrical.generation_power_w.abs() < IDLE_KW_THRESHOLD);
        assert!(slots.fuel.get(FuelType::Gas) < IDLE_KW_THRESHOLD);
    }

    // =======================================================================
    // Regression: telemetry units (all must be kW, never W)
    // =======================================================================

    #[test]
    fn telemetry_fuel_input_is_in_watts() {
        // A 10 kW generator at rated output with eta=0.30 should report:
        //   electric_output_kw = 10
        //   fuel_input_w       = (10/0.30)*1000 ≈ 33333 W
        let config = gen_config(&[]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        ramp_to_steady_state(&mut generator, 10.0, &base_env());

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        // At 10 kW electric / 0.30 eta → fuel ≈ 33333 W.
        let expected_fuel_w = (10.0 / 0.30) * 1000.0;
        assert!(
            (fuel_w - expected_fuel_w).abs() < 1.0,
            "fuel_input_w={fuel_w}, expected≈{expected_fuel_w}"
        );
    }

    // =======================================================================
    // Regression: load_state telemetry completeness
    // =======================================================================

    #[test]
    fn load_state_restores_derived_telemetry() {
        let config = gen_config(&[]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        ramp_to_steady_state(&mut generator, 10.0, &base_env());

        // Run one step to populate all telemetry fields.
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let bytes = generator.save_state().unwrap();

        let mut generator2 = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator2.init(&config, &base_env()).unwrap();
        generator2.load_state(&bytes).unwrap();

        let fuel_w = generator2.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let eta = generator2.telemetry().get(tk::ETA_ELECTRIC).unwrap();

        assert!(
            fuel_w > 0.0,
            "fuel_input_w should be non-zero after load_state; got {fuel_w}"
        );
        assert!(
            eta > 0.0,
            "eta_electric should be non-zero after load_state; got {eta}"
        );

        // Consistency: fuel_w ≈ (electric_kw * 1000) / eta
        let electric_kw = generator2.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap();
        let expected_fuel_w = (electric_kw * 1000.0) / eta;
        assert!(
            (fuel_w - expected_fuel_w).abs() < 1.0,
            "fuel_input_w ({fuel_w}) inconsistent with electric_output_kw ({electric_kw}) * 1000 / eta ({eta})"
        );
    }

    // =======================================================================
    // Telemetry field names are in watts (regression tests)
    // =======================================================================

    #[test]
    fn telemetry_field_names_are_fuel_input_w_thermal_output_w_flue_loss_w() {
        // Verify the exact field names -- old kW names must NOT appear.
        let generator_no_chp = Generator::new(gen_config(&[]), GeneratorKind::GasGenerator);
        let names_no_chp: Vec<&str> = generator_no_chp
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(
            names_no_chp.contains(&tk::FUEL_INPUT_W),
            "must use fuel_input_w"
        );
        assert!(
            !names_no_chp.contains(&"fuel_input_kw"),
            "old kW name must not appear"
        );
        assert!(
            !names_no_chp.contains(&"thermal_output_kw"),
            "old kW name must not appear"
        );
        assert!(
            !names_no_chp.contains(&"flue_loss_kw"),
            "old kW name must not appear"
        );

        let generator_chp = Generator::new(
            gen_config(&[(KEY_ETA_THERMAL, 0.30.into()), (KEY_ZONE_ID, 1.0.into())]),
            GeneratorKind::GasGenerator,
        );
        let names_chp: Vec<&str> = generator_chp
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(names_chp.contains(&tk::FUEL_INPUT_W));
        assert!(
            names_chp.contains(&tk::THERMAL_OUTPUT_W),
            "must use thermal_output_w"
        );
        assert!(names_chp.contains(&tk::FLUE_LOSS_W), "must use flue_loss_w");
    }

    #[test]
    fn telemetry_values_are_in_watts_not_kilowatts() {
        // A 5 kW generator with eta=0.25 should report fuel_input_w = 5000/0.25 = 20000 W.
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.25.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
            (KEY_ETA_THERMAL, 0.30.into()),
            (KEY_ZONE_ID, 1.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let thermal_w = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let flue_w = generator.telemetry().get(tk::FLUE_LOSS_W).unwrap();

        // fuel_input_w should be 5000 / 0.25 = 20000 W
        let expected_fuel_w = 5000.0 / 0.25;
        assert!(
            (fuel_w - expected_fuel_w).abs() < 1.0,
            "fuel_input_w={fuel_w}, expected={expected_fuel_w} (watts)"
        );
        // thermal_output_w should be fuel_w * eta_thermal = 20000 * 0.30 = 6000 W
        let expected_thermal_w = fuel_w * 0.30;
        assert!(
            (thermal_w - expected_thermal_w).abs() < 1.0,
            "thermal_output_w={thermal_w}, expected={expected_thermal_w} (watts)"
        );
        // flue_loss_w = fuel_w - electric_w - thermal_w = 20000 - 5000 - 6000 = 9000 W
        let expected_flue_w = fuel_w - 5000.0 - thermal_w;
        assert!(
            (flue_w - expected_flue_w).abs() < 1.0,
            "flue_loss_w={flue_w}, expected={expected_flue_w} (watts)"
        );
    }

    // =======================================================================
    // FuelCell defaults to curve efficiency
    // =======================================================================

    #[test]
    fn fuel_cell_defaults_to_curve_efficiency() {
        // Create a FuelCell without specifying efficiency_type -- it must default to Curve,
        // not Constant. For FuelCell, the DC stack power is higher than AC because of
        // inverter losses (default inverter_efficiency = 0.95), so the capacity_ratio
        // at 2.5 kW AC is 2.5/0.95/10 = 0.263, giving eta ≈ 0.5.
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.95.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut fc = Generator::new(config.clone(), GeneratorKind::FuelCell);
        fc.init(&config, &base_env()).unwrap();

        // At cr=1.0 DC (≈9.5 kW AC for fuel cell), curve and constant both return rated.
        // At cr≈0.263 (2.5 kW AC), OCHRE default curve gives rated * 0.526; constant gives rated.
        fc.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 2.5,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
        let mut slots = ports_for(&fc);
        fc.step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let eta = fc.telemetry().get(tk::ETA_ELECTRIC).unwrap();
        // Fuel cell DC power = 2.5 / 0.95 = 2.6316 kW, cr = 0.26316
        // Curve interp on OCHRE default (0,0)-(0.5,1): er = 0.5263, eta = 0.95*0.5263 ≈ 0.5
        // Constant at any cr: eta = 0.95
        assert!(
            (eta - 0.5).abs() < 1e-2,
            "FuelCell at 2.5 kW AC (cr≈0.263) should use curve efficiency (eta≈0.5), got {eta}. \
             This indicates the default is Constant rather than Curve."
        );
    }

    // GeneratorConfig typed round-trip and validation tests

    fn minimal_generator_config() -> GeneratorConfig {
        GeneratorConfig {
            equipment_id: None,
            zone_id: None,
            fuel_type: None,
            rated_power_kw: 10.0,
            eta_electric: None,
            eta_thermal: None,
            eta_jacket_water: None,
            eta_lube_oil: None,
            eta_exhaust: None,
            efficiency_type: None,
            efficiency_curve_points: None,
            delta_kw_per_s: None,
            capacity_min_kw: None,
            grid_import_limit_kw: None,
            export_limit_kw: None,
            loop_id: None,
            flow_rate_kg_s: None,
            supply_temp_c: None,
            return_temp_c: None,
            inverter_efficiency: None,
            stack_temp_c: None,
            stack_cooler_r0: None,
            stack_cooler_r1: None,
            stack_cooler_r2: None,
            stack_cooler_r3: None,
            stack_nominal_temp_c: None,
            heat_rec_max_temp_c: None,
        }
    }

    #[test]
    fn generator_config_round_trips_via_equipment_config() {
        let cfg = minimal_generator_config();
        let ec = EquipmentConfig::from_typed(
            "test_gen".to_string(),
            "Gas Generator".to_string(),
            cfg.clone(),
        );
        assert!(ec.is_typed());
        let recovered: GeneratorConfig = ec.typed().unwrap();
        assert_eq!(recovered.rated_power_kw, cfg.rated_power_kw);
    }

    #[test]
    fn generator_config_round_trips_efficiency_curve_points() {
        let mut cfg = minimal_generator_config();
        cfg.efficiency_type = Some("curve".to_string());
        cfg.efficiency_curve_points = Some(vec![
            GeneratorEfficiencyCurvePoint {
                capacity_ratio: 0.0,
                efficiency_ratio: 0.0,
            },
            GeneratorEfficiencyCurvePoint {
                capacity_ratio: 0.5,
                efficiency_ratio: 0.9,
            },
            GeneratorEfficiencyCurvePoint {
                capacity_ratio: 1.0,
                efficiency_ratio: 1.0,
            },
        ]);
        let ec = EquipmentConfig::from_typed(
            "test_gen".to_string(),
            "Gas Generator".to_string(),
            cfg.clone(),
        );
        let recovered: GeneratorConfig = ec.typed().unwrap();
        assert_eq!(
            recovered.efficiency_curve_points,
            cfg.efficiency_curve_points
        );
    }

    #[test]
    fn generator_config_rejects_unknown_fields() {
        use crate::config::ConfigPayload;
        let json = serde_json::json!({
            "rated_power_kw": 10.0,
            "mystery_field": "oops"
        });
        let ec = EquipmentConfig::with_payload(
            "gen".to_string(),
            "Gas Generator".to_string(),
            ConfigPayload::Typed {
                type_name: "Generator".to_string(),
                version: 1,
                data: json,
            },
        );
        let result: crate::Result<GeneratorConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn generator_config_validate_rejects_zero_rated_power() {
        let mut cfg = minimal_generator_config();
        cfg.rated_power_kw = 0.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn generator_config_validate_rejects_efficiency_sum_exceeding_1() {
        let mut cfg = minimal_generator_config();
        cfg.eta_electric = Some(0.7);
        cfg.eta_thermal = Some(0.5);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn generator_config_validate_rejects_curve_with_too_few_points() {
        let mut cfg = minimal_generator_config();
        cfg.efficiency_type = Some("curve".to_string());
        cfg.efficiency_curve_points = Some(vec![GeneratorEfficiencyCurvePoint {
            capacity_ratio: 0.0,
            efficiency_ratio: 0.0,
        }]);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn generator_config_validate_rejects_curve_with_non_monotonic_capacity_ratio() {
        let mut cfg = minimal_generator_config();
        cfg.efficiency_type = Some("curve".to_string());
        cfg.efficiency_curve_points = Some(vec![
            GeneratorEfficiencyCurvePoint {
                capacity_ratio: 0.0,
                efficiency_ratio: 0.0,
            },
            GeneratorEfficiencyCurvePoint {
                capacity_ratio: 0.6,
                efficiency_ratio: 0.8,
            },
            GeneratorEfficiencyCurvePoint {
                capacity_ratio: 0.5,
                efficiency_ratio: 1.0,
            },
        ]);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn generator_config_validate_rejects_curve_capacity_ratio_out_of_range() {
        let mut cfg = minimal_generator_config();
        cfg.efficiency_type = Some("curve".to_string());
        cfg.efficiency_curve_points = Some(vec![
            GeneratorEfficiencyCurvePoint {
                capacity_ratio: 0.0,
                efficiency_ratio: 0.0,
            },
            GeneratorEfficiencyCurvePoint {
                capacity_ratio: 1.1,
                efficiency_ratio: 1.0,
            },
        ]);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn generator_config_validate_passes_for_valid_chp() {
        let mut cfg = minimal_generator_config();
        cfg.eta_electric = Some(0.35);
        cfg.eta_thermal = Some(0.45);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn core_output_operating_mode_reflects_generator_state() {
        let config = gen_config(&[(KEY_DELTA_KW_PER_S, 100.0.into())]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert_eq!(
            generator.core_output().state.operating_mode,
            Some(OperatingMode::Off),
            "idle generator must report Off"
        );

        ramp_to_steady_state(&mut generator, 5.0, &base_env());
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert_eq!(
            generator.core_output().state.operating_mode,
            Some(OperatingMode::Standby),
            "running generator must report Standby"
        );
    }

    #[test]
    fn typed_init_uses_fuel_type_enum_directly() {
        let cfg = GeneratorConfig {
            fuel_type: Some(FuelType::Propane),
            ..minimal_generator_config()
        };
        let ec =
            EquipmentConfig::from_typed("typed_gen".to_string(), "Gas Generator".to_string(), cfg);
        let env = base_env();
        let mut r#gen = Generator::new(ec.clone(), GeneratorKind::GasGenerator);
        r#gen.init(&ec, &env).expect("typed init");

        assert_eq!(r#gen.descriptor().fuel, FuelType::Propane);
    }

    // =======================================================================
    // Fuel-cell-specific physics tests
    // =======================================================================

    #[test]
    fn fuel_cell_dc_power_exceeds_ac_power_by_inverter_loss() {
        // A FuelCell with inverter_efficiency < 1.0 must produce more DC
        // than AC to compensate for inverter conversion losses.
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.50.into()),
            (KEY_INVERTER_EFFICIENCY, 0.90.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut fc = Generator::new(config.clone(), GeneratorKind::FuelCell);
        fc.init(&config, &base_env()).unwrap();

        fc.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
        let mut slots = ports_for(&fc);
        fc.step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let ac_kw = fc.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap();
        let dc_kw = fc.telemetry().get(tk::FUEL_CELL_DC_KW).unwrap();
        let inv_loss_w = fc.telemetry().get(tk::FUEL_CELL_INVERTER_LOSS_W).unwrap();

        // AC output matches the setpoint.
        assert!((ac_kw - 5.0).abs() < 1e-9, "AC output should be 5 kW");
        // DC must be higher than AC because inverter has losses.
        assert!(dc_kw > ac_kw, "DC ({dc_kw} kW) must exceed AC ({ac_kw} kW)");
        // P_ac = P_dc * inverter_efficiency
        assert!(
            (ac_kw - dc_kw * 0.90).abs() < 1e-9,
            "P_ac = P_dc * inverter_eta: {ac_kw} != {dc_kw} * 0.90"
        );
        // Inverter loss = DC - AC
        let expected_loss = (dc_kw - ac_kw) * 1000.0;
        assert!(
            (inv_loss_w - expected_loss).abs() < 1.0,
            "inverter_loss_w={inv_loss_w}, expected={expected_loss}"
        );
    }

    #[test]
    fn fuel_cell_stack_heat_polynomial_matches_energyplus() {
        // EnergyPlus FuelCellElectricGenerator.cc:1862:
        //   qs_cool = (r0 + r1*(Tstack - Tnom)) * (1 + r2*Pel + r3*Pel^2) * Pel
        // With r0=0.2, r1=0.0, r2=0.0, r3=0.0:
        //   qs_cool = 0.2 * Pel (20 % of DC power as stack heat)
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.50.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
            (KEY_STACK_COOLER_R0, 0.20.into()),
            (KEY_STACK_COOLER_R1, 0.0.into()),
            (KEY_STACK_COOLER_R2, 0.0.into()),
            (KEY_STACK_COOLER_R3, 0.0.into()),
            (KEY_STACK_NOMINAL_TEMP_C, 70.0.into()),
            (KEY_STACK_TEMP_C, 70.0.into()),
        ]);
        let mut fc = Generator::new(config.clone(), GeneratorKind::FuelCell);
        fc.init(&config, &base_env()).unwrap();

        fc.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
        let mut slots = ports_for(&fc);
        fc.step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let dc_kw = fc.telemetry().get(tk::FUEL_CELL_DC_KW).unwrap();
        let stack_heat_w = fc.telemetry().get(tk::FUEL_CELL_STACK_HEAT_W).unwrap();
        // With r0=0.2 and all other coeffs zero: qs_cool = 0.2 * P_dc_W
        let expected_heat_w = 0.20 * dc_kw * 1000.0;
        assert!(
            (stack_heat_w - expected_heat_w).abs() < 1.0,
            "stack cooling: {stack_heat_w} W, expected {expected_heat_w} W"
        );
    }

    #[test]
    fn fuel_cell_stack_heat_scales_with_temperature_offset() {
        // When Tstack > Tnom and r1 > 0, stack heat increases with temperature.
        // qs_cool = (r0 + r1*(Tstack - Tnom)) * Pel (with r2=r3=0)
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.50.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
            (KEY_STACK_COOLER_R0, 0.10.into()),
            (KEY_STACK_COOLER_R1, 0.005.into()), // 0.5%/°C
            (KEY_STACK_COOLER_R2, 0.0.into()),
            (KEY_STACK_COOLER_R3, 0.0.into()),
            (KEY_STACK_NOMINAL_TEMP_C, 70.0.into()),
            (KEY_STACK_TEMP_C, 80.0.into()), // 10°C above nominal
        ]);
        let mut fc = Generator::new(config.clone(), GeneratorKind::FuelCell);
        fc.init(&config, &base_env()).unwrap();

        fc.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
        let mut slots = ports_for(&fc);
        fc.step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let dc_kw = fc.telemetry().get(tk::FUEL_CELL_DC_KW).unwrap();
        let stack_heat_w = fc.telemetry().get(tk::FUEL_CELL_STACK_HEAT_W).unwrap();
        // (r0 + r1*10) * Pel = (0.10 + 0.005*10) * Pel = 0.15 * Pel
        let expected_heat_w = (0.10 + 0.005 * 10.0) * dc_kw * 1000.0;
        assert!(
            (stack_heat_w - expected_heat_w).abs() < 1.0,
            "stack cooling with temp offset: {stack_heat_w} W, expected {expected_heat_w} W"
        );
    }

    #[test]
    fn fuel_cell_stack_heat_r2_coefficient_uses_watt_units() {
        // Regression: verify that r2 and r3 use 1/W and 1/W² (EnergyPlus convention).
        // Before the fix, the function accepted kW causing r2/r3 to be off by 1e3/1e6.
        // r2=2e-5 (1/W), Pel=~5263 W → r2*Pel = 0.1053 → power_factor = 1.1053
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.50.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
            (KEY_STACK_COOLER_R0, 0.20.into()),
            (KEY_STACK_COOLER_R1, 0.0.into()),
            (KEY_STACK_COOLER_R2, 2e-5.into()),
            (KEY_STACK_COOLER_R3, 0.0.into()),
            (KEY_STACK_NOMINAL_TEMP_C, 70.0.into()),
            (KEY_STACK_TEMP_C, 70.0.into()),
            (KEY_INVERTER_EFFICIENCY, 0.95.into()),
        ]);
        let mut fc = Generator::new(config.clone(), GeneratorKind::FuelCell);
        fc.init(&config, &base_env()).unwrap();

        fc.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
        let mut slots = ports_for(&fc);
        fc.step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let dc_kw = fc.telemetry().get(tk::FUEL_CELL_DC_KW).unwrap();
        let stack_heat_w = fc.telemetry().get(tk::FUEL_CELL_STACK_HEAT_W).unwrap();
        // Pel = dc_kw * 1000, power_factor = 1 + r2*Pel = 1 + 2e-5 * Pel
        let pel_w = dc_kw * 1000.0;
        let power_factor = 1.0 + 2e-5 * pel_w;
        let expected_heat_w = 0.20 * power_factor * pel_w;
        assert!(
            (stack_heat_w - expected_heat_w).abs() < 1.0,
            "r2=2e-5: stack_heat={stack_heat_w} W, expected={expected_heat_w} W (dc={dc_kw} kW, pel={pel_w} W, pf={power_factor})"
        );
    }

    #[test]
    fn fuel_cell_telemetry_fields_registered() {
        // A FuelCell must register three fuel-cell-specific telemetry fields.
        let fc = Generator::new(gen_config(&[]), GeneratorKind::FuelCell);
        let names: Vec<&str> = fc
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        for expected in &[
            tk::FUEL_CELL_DC_KW,
            tk::FUEL_CELL_INVERTER_LOSS_W,
            tk::FUEL_CELL_STACK_HEAT_W,
        ] {
            assert!(
                names.contains(expected),
                "FuelCell missing telemetry field: {expected}"
            );
        }
    }

    #[test]
    fn fuel_cell_fuel_consumption_includes_inverter_loss() {
        // With inverter_efficiency < 1.0, a FuelCell requires more fuel per kW
        // of AC output than a GasGenerator with the same eta_electric.
        let config_gg = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.50.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let config_fc = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.50.into()),
            (KEY_INVERTER_EFFICIENCY, 0.90.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);

        let mut gg = Generator::new(config_gg.clone(), GeneratorKind::GasGenerator);
        gg.init(&config_gg, &base_env()).unwrap();
        let mut fc = Generator::new(config_fc.clone(), GeneratorKind::FuelCell);
        fc.init(&config_fc, &base_env()).unwrap();

        // Run GasGenerator at 5 kW AC with eta=0.50.
        gg.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
        gg.step(&base_env(), Duration::from_secs(1), &mut ports_for(&gg))
            .unwrap();

        // Run FuelCell at 5 kW AC with eta=0.50, inverter=0.90.
        fc.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();
        fc.step(&base_env(), Duration::from_secs(1), &mut ports_for(&fc))
            .unwrap();

        let fuel_gg_w = gg.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let fuel_fc_w = fc.telemetry().get(tk::FUEL_INPUT_W).unwrap();

        // GasGenerator fuel = 5 kW AC / 0.50 = 10000 W
        assert!((fuel_gg_w - 10_000.0).abs() < 1.0);
        // FuelCell fuel = (5 kW / 0.90) DC / 0.50 = 5.5556 kW / 0.50 = 11111 W
        assert!(
            fuel_fc_w > fuel_gg_w,
            "FuelCell should use more fuel due to inverter loss"
        );
        assert!(
            (fuel_fc_w - 11_111.1111).abs() < 20.0,
            "FuelCell fuel should be ~11111 W, got {fuel_fc_w}"
        );
    }

    #[test]
    fn gas_generator_unchanged_by_fuel_cell_changes() {
        // Regression: A GasGenerator with default config must produce the same
        // results as before the fuel cell physics were added (inverter_efficiency=1.0).
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.30.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        // 5 kW AC / 0.30 eta = 16666.67 W fuel
        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let expected = (5.0 / 0.30) * 1000.0;
        assert!(
            (fuel_w - expected).abs() < 1.0,
            "GasGenerator fuel unchanged: {fuel_w} vs {expected}"
        );
        // No fuel cell telemetry on gas generator — fields not registered.
        assert!(
            generator.telemetry().get(tk::FUEL_CELL_DC_KW).is_none(),
            "GasGenerator should not register fuel_cell_dc_kw"
        );
        assert!(
            generator
                .telemetry()
                .get(tk::FUEL_CELL_INVERTER_LOSS_W)
                .is_none(),
            "GasGenerator should not register fuel_cell_inverter_loss_w"
        );
    }

    #[test]
    fn fuel_cell_save_load_state_preserves_telemetry() {
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.50.into()),
            (KEY_INVERTER_EFFICIENCY, 0.90.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut fc = Generator::new(config.clone(), GeneratorKind::FuelCell);
        fc.init(&config, &base_env()).unwrap();
        ramp_to_steady_state(&mut fc, 5.0, &base_env());

        let mut slots = ports_for(&fc);
        fc.step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        let bytes = fc.save_state().unwrap();

        let mut fc2 = Generator::new(config.clone(), GeneratorKind::FuelCell);
        fc2.init(&config, &base_env()).unwrap();
        fc2.load_state(&bytes).unwrap();

        assert!((fc2.telemetry().get(tk::FUEL_CELL_DC_KW).unwrap() - 5.0 / 0.90).abs() < 1e-6);
        assert!(fc2.telemetry().get(tk::FUEL_CELL_INVERTER_LOSS_W).unwrap() > 0.0);
        assert!(fc2.telemetry().get(tk::FUEL_CELL_STACK_HEAT_W).unwrap() > 0.0);
    }

    // =======================================================================
    // Heat recovery temperature capping
    // =======================================================================

    #[test]
    fn heat_rec_capping_limits_outlet_to_max_temp() {
        // EnergyPlus HRecRatio: when loop return temp + temp_rise > max_temp,
        // thermal output is scaled down so the loop outlet stays at or below max.
        // With return_temp_c = 75°C, max_temp = 80°C, flow = 0.1 kg/s, Cp = 4180 J/(kg·K):
        // max absorbable = 0.1 * 4180 * (80 - 75) = 2090 W.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.20.into()),
            (KEY_ETA_EXHAUST, 0.30.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_LOOP_ID, 7.0.into()),
            (KEY_FLOW_RATE_KG_S, 0.1.into()),
            (KEY_RETURN_TEMP_C, 75.0.into()),
            (KEY_HEAT_REC_MAX_TEMP_C, 80.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let q_available = generator.telemetry().get(tk::THERMAL_AVAILABLE_W).unwrap();
        let q_effective = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let ratio = generator.telemetry().get(tk::HEAT_REC_RATIO).unwrap();
        let loop_return = generator.telemetry().get(tk::LOOP_RETURN_TEMP_C).unwrap();

        assert!(
            q_available > 0.0,
            "thermal_available should be positive: {q_available}"
        );
        assert!(
            q_effective > 0.0,
            "thermal_effective should be positive: {q_effective}"
        );
        assert!(
            q_effective < q_available,
            "thermal_effective ({q_effective}) should be less than thermal_available ({q_available}) when capping is active"
        );
        assert!(
            ratio < 1.0,
            "heat_rec_ratio should be less than 1.0 with capping, got {ratio}"
        );
        assert!(
            ratio > 0.0,
            "heat_rec_ratio should be greater than 0.0, got {ratio}"
        );
        assert!(
            (loop_return - 75.0).abs() < 1e-9,
            "loop_return_temp_c should be 75°C, got {loop_return}"
        );

        // Verify loop outlet temperature does not exceed max_temp.
        // outlet = return + q_effective / (flow * Cp)
        let cp = CP_LIQUID_WATER_J_KG_K;
        let outlet_temp_c = loop_return + q_effective / (0.1 * cp);
        assert!(
            outlet_temp_c <= 80.0 + 1e-9,
            "loop outlet temperature ({outlet_temp_c}) must not exceed max_temp (80°C)"
        );

        // Verify invariant: 0.0 <= ratio <= 1.0 (checked by debug_assert in step)
        assert!((0.0..=1.0).contains(&ratio));
        assert!(q_effective <= q_available);
    }

    #[test]
    fn heat_rec_ratio_is_one_when_no_temperature_constraint() {
        // Without heat_rec_max_temp_c, there is no capping.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.20.into()),
            (KEY_ETA_EXHAUST, 0.30.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_LOOP_ID, 7.0.into()),
            (KEY_FLOW_RATE_KG_S, 0.1.into()),
            (KEY_RETURN_TEMP_C, 60.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let q_available = generator.telemetry().get(tk::THERMAL_AVAILABLE_W).unwrap();
        let q_effective = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let ratio = generator.telemetry().get(tk::HEAT_REC_RATIO).unwrap();

        assert!(
            (ratio - 1.0).abs() < f64::EPSILON,
            "heat_rec_ratio should be 1.0 without max_temp, got {ratio}"
        );
        assert!(
            (q_effective - q_available).abs() < 1.0,
            "effective thermal should equal available thermal without capping"
        );
    }

    #[test]
    fn heat_rec_ratio_is_one_when_max_temp_is_none() {
        // Explicitly set heat_rec_max_temp_c = None via config.
        // This is the default; test verifies no-capping behaviour.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.20.into()),
            (KEY_ETA_EXHAUST, 0.30.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_LOOP_ID, 7.0.into()),
            (KEY_FLOW_RATE_KG_S, 0.1.into()),
            (KEY_RETURN_TEMP_C, 75.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let ratio = generator.telemetry().get(tk::HEAT_REC_RATIO).unwrap();
        assert!(
            (ratio - 1.0).abs() < f64::EPSILON,
            "heat_rec_ratio should be 1.0 when max_temp is None, got {ratio}"
        );
    }

    #[test]
    fn existing_chp_tests_produce_identical_results_without_max_temp() {
        // Regression: without heat_rec_max_temp_c, thermal_output should match the
        // uncapped formula (q_thermal = fuel_w * eta_thermal) exactly as before.
        let config = gen_config(&[
            (KEY_ETA_THERMAL, 0.40.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 6.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let thermal_w = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let thermal_avail = generator.telemetry().get(tk::THERMAL_AVAILABLE_W).unwrap();

        // Q_thermal = P_fuel * eta_thermal (uncapped formula)
        assert!(
            (thermal_w - fuel_w * 0.40).abs() < 1.0,
            "thermal_output should match legacy formula: got {thermal_w}, expected {}",
            fuel_w * 0.40
        );
        // Available must equal effective when not capped.
        assert!(
            (thermal_w - thermal_avail).abs() < 1.0,
            "thermal_available ({thermal_avail}) must equal thermal_output ({thermal_w}) when no capping"
        );
    }

    #[test]
    fn heat_rec_telemetry_fields_registered_when_chp_active() {
        // New telemetry fields must be registered when CHP is active.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.15.into()),
            (KEY_ETA_EXHAUST, 0.25.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_LOOP_ID, 7.0.into()),
            (KEY_HEAT_REC_MAX_TEMP_C, 85.0.into()),
        ]);
        let generator = Generator::new(config, GeneratorKind::GasGenerator);
        let names: Vec<&str> = generator
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        for expected in &[
            tk::THERMAL_AVAILABLE_W,
            tk::HEAT_REC_RATIO,
            tk::LOOP_RETURN_TEMP_C,
        ] {
            assert!(
                names.contains(expected),
                "missing telemetry field: {expected}"
            );
        }
    }

    #[test]
    fn heat_rec_ratio_is_zero_when_loop_already_saturated() {
        // When the loop return temperature already equals or exceeds the max temp,
        // the loop cannot absorb any heat without immediately exceeding the cap.
        // EnergyPlus ICEngineElectricGenerator.cc:785: HRecRatio = 0.0 when
        // HeatRecInTemp == HeatRecMaxTemp (MinHeatRecMdot = 0.0).
        let cases = [
            (80.0, "return_temp exactly at max_temp"),
            (82.0, "return_temp above max_temp"),
        ];
        for &(return_temp, label) in &cases {
            let config = gen_config(&[
                (KEY_ETA_JACKET_WATER, 0.20.into()),
                (KEY_ETA_EXHAUST, 0.30.into()),
                (KEY_ZONE_ID, 1.0.into()),
                (KEY_LOOP_ID, 7.0.into()),
                (KEY_FLOW_RATE_KG_S, 0.1.into()),
                (KEY_RETURN_TEMP_C, return_temp.into()),
                (KEY_HEAT_REC_MAX_TEMP_C, 80.0.into()),
                (KEY_DELTA_KW_PER_S, 100.0.into()),
            ]);
            let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
            generator.init(&config, &base_env()).unwrap();
            generator
                .apply_control(&ControlSignal::PowerSetpoint {
                    active_power_kw: 10.0,
                    reactive_power_kvar: None,
                    min_soc: None,
                    max_soc: None,
                })
                .unwrap();
            let mut slots = ports_for(&generator);
            generator
                .step(&base_env(), Duration::from_secs(1), &mut slots)
                .unwrap();

            let q_available = generator.telemetry().get(tk::THERMAL_AVAILABLE_W).unwrap();
            let q_effective = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
            let ratio = generator.telemetry().get(tk::HEAT_REC_RATIO).unwrap();

            assert!(
                q_available > 0.0,
                "[{label}] thermal_available should be positive: {q_available}"
            );
            // Loop return is at or above max temp — all heat must be rejected.
            assert!(
                (ratio - 0.0).abs() < f64::EPSILON,
                "[{label}] heat_rec_ratio should be 0.0 when loop is saturated, got {ratio}"
            );
            assert!(
                q_effective < 1.0,
                "[{label}] thermal_effective should be near zero ({q_effective}) when loop is saturated"
            );
        }
    }

    #[test]
    fn rejected_heat_routed_to_flue_loss() {
        // When thermal output is capped, the rejected heat must be added to q_flue.
        // This verifies that energy balance holds: q_available = q_effective + q_rejected,
        // and q_rejected appears in q_flue.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.20.into()),
            (KEY_ETA_EXHAUST, 0.30.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_LOOP_ID, 7.0.into()),
            (KEY_FLOW_RATE_KG_S, 0.1.into()),
            (KEY_RETURN_TEMP_C, 75.0.into()),
            (KEY_HEAT_REC_MAX_TEMP_C, 80.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let electric_w = generator.telemetry().get(tk::ELECTRIC_OUTPUT_KW).unwrap() * 1000.0;
        let q_available = generator.telemetry().get(tk::THERMAL_AVAILABLE_W).unwrap();
        let q_effective = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let q_flue = generator.telemetry().get(tk::FLUE_LOSS_W).unwrap();
        let q_jacket = generator.telemetry().get(tk::JACKET_WATER_W).unwrap();
        let q_exhaust = generator.telemetry().get(tk::EXHAUST_WATER_W).unwrap();

        // Energy balance: fuel = electric + q_jacket + q_exhaust + q_flue
        // (q_lube defaults to 0 here)
        let balance = fuel_w - electric_w - q_jacket - q_exhaust - q_flue;
        assert!(
            balance.abs() < 1.0,
            "energy balance violated: residual={balance:.3} W"
        );

        // Rejected heat appears in q_flue.
        let q_rejected = q_available - q_effective;
        assert!(
            q_rejected > 0.0,
            "rejected heat should be positive when capping is active"
        );

        // flue_loss with capping should be larger than without (contains rejected heat).
        // The non-capped flue would be: fuel_w - electric_w - q_available
        let uncapped_flue = fuel_w - electric_w - q_available;
        assert!(
            q_flue > uncapped_flue,
            "flue loss with capping ({q_flue:.1} W) should exceed uncapped flue ({uncapped_flue:.1} W)"
        );
    }

    // =======================================================================
    // T-0084: Fluid port thermal_power_w routing
    // =======================================================================

    #[test]
    fn chp_fluid_port_carries_thermal_power_w() {
        // Unit test: generator with CHP active and a fluid port. After step(),
        // the fluid accumulator must reflect the declared thermal power.
        let config = gen_config(&[
            (KEY_ETA_THERMAL, 0.35.into()),
            (KEY_LOOP_ID, 7.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 8.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let q_thermal_w = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        assert!(q_thermal_w > 0.0, "CHP must produce thermal power");
        assert_eq!(slots.fluid.len(), 1);

        // The fluid accumulator's total_thermal_power_w should equal the generator's
        // computed thermal output since it was declared in the PortContribution.
        let delta = (slots.fluid[0].total_thermal_power_w - q_thermal_w).abs();
        assert!(
            delta < 1.0,
            "fluid accumulator total_thermal_power_w ({}) should match generator q_thermal_w ({})",
            slots.fluid[0].total_thermal_power_w,
            q_thermal_w
        );

        // Regression: the fluid port must be self-consistent — flow × Cp × ΔT
        // must equal the declared thermal_power_w. Before the fix for T-0084, the
        // generator wrote a dynamically computed thermal_power_w alongside static
        // config temperature values, causing the fluid solver invariant to fire.
        let flow_implied_w = slots.fluid[0].total_flow_kg_s
            * CP_LIQUID_WATER_J_KG_K
            * (slots.fluid[0].mean_supply_temp_c - slots.fluid[0].mean_return_temp_c);
        let mismatch = (flow_implied_w - slots.fluid[0].total_thermal_power_w).abs();
        assert!(
            mismatch < 1.0,
            "fluid port data is self-inconsistent: flow-implied energy ({flow_implied_w} W) \
             != declared thermal_power_w ({declared} W); diff = {mismatch} W",
            declared = slots.fluid[0].total_thermal_power_w
        );
    }

    #[test]
    fn chp_without_fluid_port_thermal_power_w_is_zero() {
        // Regression test: when CHP is active WITHOUT a fluid port, the accumulator
        // should show zero thermal_power_w (energy goes to zone, not fluid).
        let config = gen_config(&[
            (KEY_ETA_THERMAL, 0.40.into()),
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 6.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        // No fluid port registered → no fluid accumulator.
        assert!(
            slots.fluid.is_empty(),
            "no fluid accumulator expected without loop_id config"
        );
    }

    #[test]
    fn chp_no_thermal_recovery_fluid_port_thermal_power_w_none() {
        // Regression test: when no thermal recovery is active (eta_thermal = 0
        // and no per-stream etas), the generator does not create a fluid port
        // at all, so thermal_power_w is never populated.
        let config = gen_config(&[
            (KEY_ZONE_ID, 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        // Thermal recovery disabled → no CHP telemetry keys registered.
        assert!(
            generator.telemetry().get(tk::THERMAL_OUTPUT_W).is_none(),
            "THERMAL_OUTPUT_W should not be registered when no thermal recovery"
        );
        assert!(
            generator
                .telemetry()
                .get(tk::THERMAL_POWER_DELIVERED_W)
                .is_none(),
            "THERMAL_POWER_DELIVERED_W should not be registered when no thermal recovery"
        );
    }

    #[test]
    fn thermal_power_delivered_w_telemetry_registered_and_set() {
        // Verify that THERMAL_POWER_DELIVERED_W is registered as a telemetry field
        // and populated with the correct value when CHP is active with a fluid port.
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.15.into()),
            (KEY_LOOP_ID, 7.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        // Check telemetry field is registered at init.
        let names: Vec<&str> = generator
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(
            names.contains(&tk::THERMAL_POWER_DELIVERED_W),
            "THERMAL_POWER_DELIVERED_W telemetry field should be registered"
        );

        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 6.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let delivered = generator
            .telemetry()
            .get(tk::THERMAL_POWER_DELIVERED_W)
            .unwrap();
        let thermal_out = generator.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        assert!(
            delivered > 0.0,
            "delivered thermal power should be positive"
        );
        assert!(
            (delivered - thermal_out).abs() < 1.0,
            "thermal_power_delivered_w ({delivered}) should match thermal_output_w ({thermal_out})"
        );
    }

    // =======================================================================
    // T-0084: load_state restores thermal_power_delivered_w
    // =======================================================================

    #[test]
    fn load_state_restores_thermal_power_delivered_w() {
        let config = gen_config(&[
            (KEY_ETA_JACKET_WATER, 0.15.into()),
            (KEY_LOOP_ID, 7.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();
        ramp_to_steady_state(&mut generator, 6.0, &base_env());

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let bytes = generator.save_state().unwrap();
        let mut restored = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        restored.init(&config, &base_env()).unwrap();
        restored.load_state(&bytes).unwrap();

        let delivered = restored
            .telemetry()
            .get(tk::THERMAL_POWER_DELIVERED_W)
            .unwrap();
        assert!(
            delivered > 0.0,
            "thermal_power_delivered_w should be restored after load_state"
        );
    }
}
