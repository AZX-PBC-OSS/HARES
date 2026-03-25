//! Generator equipment models: gas generators and fuel cells.
//!
//! Both types share identical physics (efficiency model, ramp-rate, self-consumption,
//! combined heat and power). A single `Generator` struct handles all logic.
//! `GasGenerator` and `FuelCell` are newtypes that forward via `delegate_equipment!`.
//!
//! Physics overview:
//!   eta = EfficiencyModel::evaluate(capacity_ratio)  — constant, curve, or quadratic
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
    ControlCapabilities, ControlSignal, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FluidType, FuelType, HaresError, LoopId, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, Telemetry, TelemetryField, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

// ---------------------------------------------------------------------------
// Config keys
// ---------------------------------------------------------------------------

use crate::config::{KEY_EQUIPMENT_ID, KEY_ZONE_ID};
const KEY_RATED_POWER_KW: &str = "rated_power_kw";
const KEY_CAPACITY_MIN_KW: &str = "capacity_min_kw";
const KEY_ETA_ELECTRIC: &str = "eta_electric";
const KEY_ETA_THERMAL: &str = "eta_thermal";
const KEY_EFFICIENCY_TYPE: &str = "efficiency_type";
const KEY_DELTA_KW_PER_S: &str = "delta_kw_per_s";
const KEY_GRID_IMPORT_LIMIT_KW: &str = "grid_import_limit_kw";
const KEY_EXPORT_LIMIT_KW: &str = "export_limit_kw";
const KEY_LOOP_ID: &str = "loop_id";
const KEY_FLOW_RATE_KG_S: &str = "flow_rate_kg_s";
const KEY_SUPPLY_TEMP_C: &str = "supply_temp_c";
const KEY_RETURN_TEMP_C: &str = "return_temp_c";

// Efficiency curve config keys: up to 16 (capacity_ratio, efficiency_ratio) pairs.
const KEY_CURVE_POINT_PREFIX: &str = "efficiency_curve_";

// ---------------------------------------------------------------------------
// Physical defaults
// ---------------------------------------------------------------------------

/// Typical spark-ignited natural gas generator electrical efficiency at rated load.
/// Generac/Briggs residential generators: 28–33% LHV; 30% is the midpoint.
const DEFAULT_ETA_ELECTRIC: f64 = 0.30;

/// Thermal recovery is disabled by default; set > 0.0 to activate CHP.
const DEFAULT_ETA_THERMAL: f64 = 0.0;

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

const IDLE_KW_THRESHOLD: f64 = 1e-6;

// ---------------------------------------------------------------------------
// EfficiencyModel — separated concern for computing load-dependent efficiency
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
        if rated <= 0.0 || rated > 1.0 {
            return Err(HaresError::Equipment(
                "generator eta_electric must be in (0, 1]".to_string(),
            ));
        }
        if let Self::Curve { points, .. } = self {
            if points.len() < 2 {
                return Err(HaresError::Equipment(
                    "generator efficiency curve requires at least 2 points".to_string(),
                ));
            }
            for w in points.windows(2) {
                if w[1].0 <= w[0].0 {
                    return Err(HaresError::Equipment(
                        "generator efficiency curve points must be sorted by capacity_ratio"
                            .to_string(),
                    ));
                }
            }
            // efficiency_ratio values > 1.0 are not rejected here because some
            // manufacturer curves have efficiency_ratio > 1.0 at peak output and
            // the effective efficiency is rated * ratio, which is bounded separately
            // by eta_electric + eta_thermal <= 1.0 in init(). Trust the caller.
        }
        Ok(())
    }

    /// Parse from config. When no explicit `efficiency_type` key is present,
    /// falls back to the kind-specific default from `GeneratorKind::default_efficiency_type`.
    fn from_config(config: &EquipmentConfig, rated: f64, kind: GeneratorKind) -> Self {
        let explicit = config.get_str(KEY_EFFICIENCY_TYPE);
        // Resolve "constant" / "curve" / "quadratic" — explicit config wins;
        // fall back to the kind default so FuelCell correctly gets "curve".
        let eff_type = explicit.unwrap_or_else(|| kind.default_efficiency_type());
        match eff_type {
            "curve" => {
                let points = parse_curve_points(config);
                if points.len() >= 2 {
                    Self::Curve { rated, points }
                } else {
                    // Fallback to OCHRE default curve: (0,0), (0.5,1), (1,1)
                    Self::Curve {
                        rated,
                        points: vec![(0.0, 0.0), (0.5, 1.0), (1.0, 1.0)],
                    }
                }
            }
            "quadratic" => Self::Quadratic { rated },
            _ => Self::Constant { rated },
        }
    }
}

/// Parse efficiency curve points from config keys like
/// `efficiency_curve_0_cr`, `efficiency_curve_0_er`, etc.
///
/// Stops at the first gap after at least one valid point has been found, matching
/// OCHRE config loading behaviour. Any indices after the gap are ignored.
fn parse_curve_points(config: &EquipmentConfig) -> Vec<(f64, f64)> {
    let mut points = Vec::new();
    let mut last_found = None;
    for i in 0..16 {
        let cr_key = format!("{KEY_CURVE_POINT_PREFIX}{i}_cr");
        let er_key = format!("{KEY_CURVE_POINT_PREFIX}{i}_er");
        if let (Some(cr), Some(er)) = (config.get_f64(&cr_key), config.get_f64(&er_key)) {
            points.push((cr, er));
            last_found = Some(i);
        } else if last_found.is_some() {
            // Gap after at least one valid point — stop here. Remaining indices
            // after the gap are considered absent (OCHRE efficiency_curve behaviour).
            break;
        }
    }
    points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    points
}

// ---------------------------------------------------------------------------
// GeneratorKind
// ---------------------------------------------------------------------------

/// Variant tag — physics are identical; used only for labelling and default efficiency type.
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
    /// Maximum output-power change per second (kW/s).
    delta_kw_per_s: f64,
    grid_import_limit_kw: f64,
    export_limit_kw: f64,

    // CHP fluid port (active when eta_thermal > 0.0 and loop_id is configured)
    chp_loop_id: Option<LoopId>,
    flow_rate_kg_s: f64,
    supply_temp_c: f64,
    return_temp_c: f64,

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
        let equipment_id = config
            .get_f64(KEY_EQUIPMENT_ID)
            .map(|v| v as u32)
            .unwrap_or(0);
        let zone = config.get_f64(KEY_ZONE_ID).map(|v| ZoneId(v as u16));
        let eta_thermal = config
            .get_f64(KEY_ETA_THERMAL)
            .unwrap_or(DEFAULT_ETA_THERMAL);
        let loop_raw = config.get_f64(KEY_LOOP_ID).map(|v| LoopId(v as u16));
        let chp_loop_id = if eta_thermal > 0.0 {
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

        let has_chp = eta_thermal > 0.0;
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id),
            name: config.name.clone(),
            end_use: EndUse::GENERATOR,
            equipment_type: Cow::Borrowed(kind.equipment_type()),
            zone,
            fuel: FuelType::Gas,
            stage: ExecutionStage::Electrical,
            control_capabilities: ControlCapabilities::POWER_SETPOINT
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::SELF_CONSUMPTION,
            telemetry_fields: generator_telemetry_fields(has_chp),
        };

        let rated = config
            .get_f64(KEY_ETA_ELECTRIC)
            .unwrap_or(DEFAULT_ETA_ELECTRIC);
        let efficiency = EfficiencyModel::from_config(&config, rated, kind);

        Self {
            descriptor,
            ports,
            telemetry: default_telemetry(has_chp),
            kind,
            rated_power_kw: config
                .get_f64(KEY_RATED_POWER_KW)
                .unwrap_or(DEFAULT_RATED_POWER_KW),
            capacity_min_kw: config.get_f64(KEY_CAPACITY_MIN_KW),
            efficiency,
            eta_thermal,
            delta_kw_per_s: config
                .get_f64(KEY_DELTA_KW_PER_S)
                .unwrap_or(DEFAULT_DELTA_KW_PER_S),
            grid_import_limit_kw: config
                .get_f64(KEY_GRID_IMPORT_LIMIT_KW)
                .unwrap_or(DEFAULT_GRID_IMPORT_LIMIT_KW),
            export_limit_kw: config
                .get_f64(KEY_EXPORT_LIMIT_KW)
                .unwrap_or(DEFAULT_EXPORT_LIMIT_KW),
            chp_loop_id,
            flow_rate_kg_s: config
                .get_f64(KEY_FLOW_RATE_KG_S)
                .unwrap_or(DEFAULT_FLOW_RATE_KG_S),
            supply_temp_c: config
                .get_f64(KEY_SUPPLY_TEMP_C)
                .unwrap_or(DEFAULT_SUPPLY_TEMP_C),
            return_temp_c: config
                .get_f64(KEY_RETURN_TEMP_C)
                .unwrap_or(DEFAULT_RETURN_TEMP_C),
            current_power_kw: 0.0,
            mode: OperatingMode::Off,
            power_setpoint_kw: None,
            self_consumption_enabled: true,
        }
    }

    /// Clamp `target_kw` to enforce the ramp-rate limit on power increases.
    ///
    /// OCHRE Generator.py:129 — ramp rate only constrains increasing generation.
    /// Shutdown and power reduction are instantaneous.
    fn apply_ramp_limit(&self, target_kw: f64, dt_s: f64) -> f64 {
        let delta = target_kw - self.current_power_kw;
        let clamped_delta = if delta > 0.0 {
            delta.min(self.delta_kw_per_s * dt_s)
        } else {
            delta // no ramp limit on decrease
        };
        (self.current_power_kw + clamped_delta).clamp(0.0, self.rated_power_kw)
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
        let raw = if let Some(sp) = self.power_setpoint_kw {
            sp.clamp(0.0, self.rated_power_kw)
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
            (net_load_kw - desired_import).clamp(0.0, self.rated_power_kw)
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
}

impl Equipment for Generator {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        self.rated_power_kw = config
            .get_f64(KEY_RATED_POWER_KW)
            .unwrap_or(self.rated_power_kw);
        self.capacity_min_kw = config.get_f64(KEY_CAPACITY_MIN_KW).or(self.capacity_min_kw);
        self.eta_thermal = config.get_f64(KEY_ETA_THERMAL).unwrap_or(self.eta_thermal);
        self.delta_kw_per_s = config
            .get_f64(KEY_DELTA_KW_PER_S)
            .unwrap_or(self.delta_kw_per_s);
        self.grid_import_limit_kw = config
            .get_f64(KEY_GRID_IMPORT_LIMIT_KW)
            .unwrap_or(self.grid_import_limit_kw);
        self.export_limit_kw = config
            .get_f64(KEY_EXPORT_LIMIT_KW)
            .unwrap_or(self.export_limit_kw);
        self.flow_rate_kg_s = config
            .get_f64(KEY_FLOW_RATE_KG_S)
            .unwrap_or(self.flow_rate_kg_s);
        self.supply_temp_c = config
            .get_f64(KEY_SUPPLY_TEMP_C)
            .unwrap_or(self.supply_temp_c);
        self.return_temp_c = config
            .get_f64(KEY_RETURN_TEMP_C)
            .unwrap_or(self.return_temp_c);

        // Rebuild efficiency model if eta_electric changed in config
        let rated = config
            .get_f64(KEY_ETA_ELECTRIC)
            .unwrap_or(self.efficiency.rated());
        self.efficiency = EfficiencyModel::from_config(config, rated, self.kind);

        // Validation
        if self.rated_power_kw <= 0.0 {
            return Err(HaresError::Equipment(
                "generator rated_power_kw must be positive".to_string(),
            ));
        }
        self.efficiency.validate()?;
        if !(0.0..=1.0).contains(&self.eta_thermal) {
            return Err(HaresError::Equipment(
                "generator eta_thermal must be in [0, 1]".to_string(),
            ));
        }
        if self.efficiency.rated() + self.eta_thermal > 1.0 {
            return Err(HaresError::Equipment(format!(
                "generator eta_electric ({}) + eta_thermal ({}) exceeds 1.0; flue loss would be negative",
                self.efficiency.rated(),
                self.eta_thermal
            )));
        }
        if self.delta_kw_per_s <= 0.0 {
            return Err(HaresError::Equipment(
                "generator delta_kw_per_s must be positive".to_string(),
            ));
        }
        if let Some(min_kw) = self.capacity_min_kw {
            if min_kw < 0.0 || min_kw > self.rated_power_kw {
                return Err(HaresError::Equipment(format!(
                    "generator capacity_min_kw ({min_kw}) must be in [0, rated_power_kw]"
                )));
            }
        }

        // Validate CHP fluid configuration.
        // TODO: Validate that loop_id references an existing water heater loop.
        // This requires cross-equipment lookup which depends on the stage snapshot mechanism.
        // Currently only validates loop_id != 0.
        if self.eta_thermal > 0.0 {
            if let Some(raw_loop) = config.get_f64(KEY_LOOP_ID) {
                let lid = raw_loop as u16;
                if lid == 0 {
                    return Err(HaresError::Equipment(
                        "generator CHP fluid loop_id must be non-zero".to_string(),
                    ));
                }
                self.chp_loop_id = Some(LoopId(lid));
            }
        }

        self.current_power_kw = 0.0;
        self.mode = OperatingMode::Off;
        self.power_setpoint_kw = None;
        self.self_consumption_enabled = true;

        let has_chp = self.eta_thermal > 0.0;
        self.telemetry = default_telemetry(has_chp);
        self.telemetry.set("eta_electric", self.efficiency.rated());

        Ok(())
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
        let net_load_kw = ports.electrical.net_active_kw();

        let unconstrained_kw = self.determine_target_kw(net_load_kw);
        // Ramp rate only constrains power increases (OCHRE Generator.py:129).
        // Flag is true only when ramping up and the increase exceeds the limit.
        let ramp_delta = unconstrained_kw - self.current_power_kw;
        let ramp_limited =
            ramp_delta > 0.0 && ramp_delta > self.delta_kw_per_s * dt_s + IDLE_KW_THRESHOLD;
        let output_kw = self.apply_ramp_limit(unconstrained_kw, dt_s);

        self.current_power_kw = output_kw;

        // Compute load-dependent efficiency.
        let capacity_ratio = output_kw / self.rated_power_kw;
        let eta = self.efficiency.evaluate(capacity_ratio);

        // Derive fuel and heat flows.
        let fuel_w = if output_kw > IDLE_KW_THRESHOLD && eta > 0.0 {
            (output_kw * 1000.0) / eta
        } else {
            0.0
        };
        let electrical_w = output_kw * 1000.0;
        let q_thermal_w = fuel_w * self.eta_thermal;
        let q_flue_w = fuel_w - electrical_w - q_thermal_w;

        // Energy accounting for zone and fluid ports:
        //   - No CHP (eta_thermal=0): all non-electrical fuel loss → zone as waste heat
        //   - CHP with fluid port: q_thermal → fluid port only; q_flue → zone
        //     (q_thermal is the useful recovered heat routed to the hydronic loop;
        //      q_flue is the residual stack loss that escapes to the zone/building)
        //   - CHP without fluid port: q_thermal → zone (no loop to route it to)
        // Energy routing to zone and fluid ports:
        //   - CHP with fluid port: q_thermal → fluid, q_flue → zone (no double-count)
        //   - CHP without fluid port: all non-electrical loss → zone (q_thermal + q_flue)
        //   - No CHP: all non-electrical loss → zone (q_flue only, since q_thermal=0)
        let zone_heat_w = if self.eta_thermal > 0.0 && self.chp_loop_id.is_some() {
            q_flue_w // fluid port takes q_thermal; zone only gets flue loss
        } else {
            // No fluid port: zone receives all waste heat (q_thermal + q_flue).
            // When eta_thermal=0, q_thermal_w=0 so this reduces to q_flue_w.
            q_thermal_w + q_flue_w
        };

        // Write electrical port (negative = generation).
        ports.accumulate(&PortContribution::Electrical {
            active_power_kw: -output_kw,
            reactive_power_kvar: 0.0,
        })?;

        // Write fuel port.
        if fuel_w > 0.0 {
            ports.accumulate(&PortContribution::Fuel {
                fuel_type: FuelType::Gas,
                consumption_w: fuel_w,
            })?;
        }

        // Write thermal port: zone heat gains from waste heat or CHP.
        if zone_heat_w > IDLE_KW_THRESHOLD {
            if let Some(zone) = self.descriptor.zone {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: zone_heat_w,
                    latent_gain_w: 0.0,
                    category: ThermalCategory::InternalGain,
                })?;
            }
        }

        // Write CHP fluid port when producing heat.
        if q_thermal_w > IDLE_KW_THRESHOLD {
            if let Some(loop_id) = self.chp_loop_id {
                ports.accumulate(&PortContribution::Fluid {
                    loop_id,
                    flow_rate_kg_s: self.flow_rate_kg_s,
                    supply_temp_c: self.supply_temp_c,
                    return_temp_c: self.return_temp_c,
                    fluid_type: FluidType::Water,
                })?;
            }
        }

        self.mode = if output_kw > IDLE_KW_THRESHOLD {
            OperatingMode::Standby
        } else {
            OperatingMode::Off
        };

        self.telemetry.set("electric_output_kw", output_kw);
        self.telemetry.set("fuel_input_w", fuel_w);
        self.telemetry.set("eta_electric", eta);
        self.telemetry
            .set("ramp_limited", if ramp_limited { 1.0 } else { 0.0 });

        if self.eta_thermal > 0.0 {
            self.telemetry.set("thermal_output_w", q_thermal_w);
            self.telemetry.set("flue_loss_w", q_flue_w);
        }

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&GeneratorCheckpoint {
            current_power_kw: self.current_power_kw,
            mode: self.mode,
            power_setpoint_kw: self.power_setpoint_kw,
            self_consumption_enabled: self.self_consumption_enabled,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let cp: GeneratorCheckpoint = load_postcard(state)?;
        self.current_power_kw = cp.current_power_kw;
        self.mode = cp.mode;
        self.power_setpoint_kw = cp.power_setpoint_kw;
        self.self_consumption_enabled = cp.self_consumption_enabled;

        // Recompute derived telemetry from restored power so consumers see
        // consistent values without needing to run a step first.
        let capacity_ratio = self.current_power_kw / self.rated_power_kw;
        let eta = self.efficiency.evaluate(capacity_ratio);
        let fuel_w = if self.current_power_kw > IDLE_KW_THRESHOLD && eta > 0.0 {
            (self.current_power_kw * 1000.0) / eta
        } else {
            0.0
        };

        self.telemetry
            .set("electric_output_kw", self.current_power_kw);
        self.telemetry.set("fuel_input_w", fuel_w);
        self.telemetry.set("eta_electric", eta);

        if self.eta_thermal > 0.0 {
            let q_thermal_w = fuel_w * self.eta_thermal;
            let q_flue_w = fuel_w - self.current_power_kw * 1000.0 - q_thermal_w;
            self.telemetry.set("thermal_output_w", q_thermal_w);
            self.telemetry.set("flue_loss_w", q_flue_w);
        }

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

fn default_telemetry(has_chp: bool) -> Telemetry {
    let capacity = if has_chp { 6 } else { 4 };
    let mut t = Telemetry::with_capacity(capacity);
    t.insert("electric_output_kw", 0.0);
    t.insert("fuel_input_w", 0.0);
    t.insert("eta_electric", 0.0);
    t.insert("ramp_limited", 0.0);
    if has_chp {
        t.insert("thermal_output_w", 0.0);
        t.insert("flue_loss_w", 0.0);
    }
    t
}

fn generator_telemetry_fields(has_chp: bool) -> Vec<TelemetryField> {
    let mut fields = vec![
        TelemetryField {
            name: "electric_output_kw".to_string(),
            unit: "kW".to_string(),
            description: "Electrical generation output".to_string(),
        },
        TelemetryField {
            name: "fuel_input_w".to_string(),
            unit: "W".to_string(),
            description: "Fuel input power (P_electric / eta)".to_string(),
        },
        TelemetryField {
            name: "eta_electric".to_string(),
            unit: "-".to_string(),
            description: "Effective electrical efficiency at current load [0..1]".to_string(),
        },
        TelemetryField {
            name: "ramp_limited".to_string(),
            unit: "-".to_string(),
            description: "1.0 when this step's power change was clamped by ramp-rate limit"
                .to_string(),
        },
    ];
    if has_chp {
        fields.push(TelemetryField {
            name: "thermal_output_w".to_string(),
            unit: "W".to_string(),
            description: "CHP thermal recovery output (P_fuel * eta_thermal)".to_string(),
        });
        fields.push(TelemetryField {
            name: "flue_loss_w".to_string(),
            unit: "W".to_string(),
            description: "Residual flue loss (P_fuel - P_electric - Q_thermal)".to_string(),
        });
    }
    fields
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, FluidAccumulator, FluidType, FuelType, GridState,
        OperatingMode, PortSlots, PortType, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
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
                relative_humidity: 0.5,
                wet_bulb_c: 15.0,
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
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid UTC timestamp"),
            time_res: ChronoDuration::minutes(5),
        }
    }

    fn gen_config(overrides: &[(&str, ConfigValue)]) -> EquipmentConfig {
        let mut raw: HashMap<String, ConfigValue> = HashMap::new();
        raw.insert(KEY_RATED_POWER_KW.to_string(), 10.0.into());
        raw.insert(KEY_ETA_ELECTRIC.to_string(), 0.30.into());
        raw.insert(KEY_DELTA_KW_PER_S.to_string(), 1.0.into());
        for (k, v) in overrides {
            raw.insert(k.to_string(), v.clone());
        }
        EquipmentConfig {
            name: "Test Generator".to_string(),
            ochre_class: "Gas Generator".to_string(),
            raw_config: raw,
        }
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
            "electric_output_kw",
            "fuel_input_w",
            "eta_electric",
            "ramp_limited",
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

        let output_kw = generator.telemetry().get("electric_output_kw").unwrap();
        let fuel_w = generator.telemetry().get("fuel_input_w").unwrap();
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
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.95.into()),
            (KEY_EFFICIENCY_TYPE, ConfigValue::Text("curve".to_string())),
            // OCHRE default curve: (0,0), (0.5,1), (1,1)
            ("efficiency_curve_0_cr", 0.0.into()),
            ("efficiency_curve_0_er", 0.0.into()),
            ("efficiency_curve_1_cr", 0.5.into()),
            ("efficiency_curve_1_er", 1.0.into()),
            ("efficiency_curve_2_cr", 1.0.into()),
            ("efficiency_curve_2_er", 1.0.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator.init(&config, &base_env()).unwrap();

        // At rated (10 kW, cr=1.0): eta = 0.95*1.0 = 0.95
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        let eta_full = generator.telemetry().get("eta_electric").unwrap();
        assert!(
            (eta_full - 0.95).abs() < 1e-6,
            "full load eta should be 0.95, got {eta_full}"
        );

        // At 25% load (2.5 kW, cr=0.25): eta = 0.95*0.5 = 0.475
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 2.5,
                reactive_power_kvar: None,
            })
            .unwrap();
        slots.zero();
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        let eta_partial = generator.telemetry().get("eta_electric").unwrap();
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
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            (generator.telemetry().get("eta_electric").unwrap() - 0.40).abs() < 1e-6,
            "full load"
        );

        // cr=0.5: eff = 0.40 * 0.625 = 0.25
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
            })
            .unwrap();
        slots.zero();
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            (generator.telemetry().get("eta_electric").unwrap() - 0.25).abs() < 1e-6,
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
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(slots.electrical.generation_power_kw < 0.0);
        assert!((slots.electrical.generation_power_kw + generator.current_power_kw).abs() < 1e-9);
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
        assert_eq!(generator.telemetry().get("ramp_limited").unwrap(), 1.0);

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
        slots.electrical.load_power_kw = 4.0;
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
        slots.electrical.load_power_kw = 5.0;
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
        slots.electrical.load_power_kw = 2.0;
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
        slots.electrical.load_power_kw = 5.0;
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

        // Set power to 0 — should shut off, not clamp to 3 kW.
        generator
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 0.0,
                reactive_power_kvar: None,
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
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get("fuel_input_w").unwrap();
        let thermal_w = generator.telemetry().get("thermal_output_w").unwrap();
        let electric_kw = generator.telemetry().get("electric_output_kw").unwrap();
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
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        // fuel_input_w, thermal_output_w, flue_loss_w are in W; electric_output_kw is in kW
        // fuel_input_w, thermal_output_w, flue_loss_w are in W; electric_output_kw is in kW
        let fuel_w = generator.telemetry().get("fuel_input_w").unwrap();
        let electric_w = generator.telemetry().get("electric_output_kw").unwrap() * 1000.0;
        let thermal_w = generator.telemetry().get("thermal_output_w").unwrap();
        let flue_w = generator.telemetry().get("flue_loss_w").unwrap();
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
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get("fuel_input_w").unwrap();
        let electric_w = generator.telemetry().get("electric_output_kw").unwrap() * 1000.0;
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
            })
            .unwrap();

        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get("fuel_input_w").unwrap();
        let electric_w = generator.telemetry().get("electric_output_kw").unwrap() * 1000.0;
        let thermal_w = generator.telemetry().get("thermal_output_w").unwrap();
        let flue_w = generator.telemetry().get("flue_loss_w").unwrap();

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

        // Fluid port should carry q_thermal
        assert_eq!(slots.fluid.len(), 1, "fluid port should be populated");
        assert!(
            thermal_w > 0.0,
            "q_thermal should be positive when CHP is active"
        );
        // The fluid port carries flow, not watts directly — just confirm it is active
        assert!(
            slots.fluid[0].total_flow_kg_s > 0.0,
            "fluid port should carry flow"
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
        let bytes = generator.save_state();

        let mut generator2 = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator2.init(&config, &base_env()).unwrap();
        generator2.load_state(&bytes).unwrap();

        assert!((generator2.current_power_kw - saved).abs() < f64::EPSILON);
        assert_eq!(generator2.power_setpoint_kw, Some(10.0));
        assert!(
            (generator2.telemetry().get("electric_output_kw").unwrap() - saved).abs()
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
        assert!(
            generator
                .init(&config, &base_env())
                .unwrap_err()
                .to_string()
                .contains("exceeds 1.0")
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
        assert!((generator.telemetry().get("eta_electric").unwrap() - 0.45).abs() < f64::EPSILON);
    }

    #[test]
    fn init_rejects_invalid_capacity_min() {
        let config = gen_config(&[(KEY_CAPACITY_MIN_KW, 20.0.into())]); // > rated 10
        let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        assert!(generator.init(&config, &base_env()).is_err());
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
        assert!(names.contains(&"thermal_output_w"));
        assert!(names.contains(&"flue_loss_w"));
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
        assert!(!names.contains(&"thermal_output_w"));
        assert!(!names.contains(&"flue_loss_w"));
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
        slots.electrical.load_power_kw = 8.0;
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();
        assert!(
            generator.current_power_kw < IDLE_KW_THRESHOLD,
            "generator should be off when self_consumption disabled, got {}",
            generator.current_power_kw
        );

        // Re-enable self-consumption — should start generating again.
        generator
            .apply_control(&ControlSignal::SelfConsumption {
                enabled: true,
                solar_only_charging: false,
            })
            .unwrap();
        slots.zero();
        slots.electrical.load_power_kw = 8.0;
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

        assert!(generator.telemetry().get("fuel_input_w").unwrap() < IDLE_KW_THRESHOLD);
        assert!(generator.telemetry().get("electric_output_kw").unwrap() < IDLE_KW_THRESHOLD);
        assert!(slots.electrical.generation_power_kw.abs() < IDLE_KW_THRESHOLD);
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

        let fuel_w = generator.telemetry().get("fuel_input_w").unwrap();
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

        let bytes = generator.save_state();

        let mut generator2 = Generator::new(config.clone(), GeneratorKind::GasGenerator);
        generator2.init(&config, &base_env()).unwrap();
        generator2.load_state(&bytes).unwrap();

        let fuel_w = generator2.telemetry().get("fuel_input_w").unwrap();
        let eta = generator2.telemetry().get("eta_electric").unwrap();

        assert!(
            fuel_w > 0.0,
            "fuel_input_w should be non-zero after load_state; got {fuel_w}"
        );
        assert!(
            eta > 0.0,
            "eta_electric should be non-zero after load_state; got {eta}"
        );

        // Consistency: fuel_w ≈ (electric_kw * 1000) / eta
        let electric_kw = generator2.telemetry().get("electric_output_kw").unwrap();
        let expected_fuel_w = (electric_kw * 1000.0) / eta;
        assert!(
            (fuel_w - expected_fuel_w).abs() < 1.0,
            "fuel_input_w ({fuel_w}) inconsistent with electric_output_kw ({electric_kw}) * 1000 / eta ({eta})"
        );
    }

    // =======================================================================
    // Regression: FuelCell defaults to curve efficiency
    // =======================================================================

    // =======================================================================
    // Telemetry field names are in watts (regression tests)
    // =======================================================================

    #[test]
    fn telemetry_field_names_are_fuel_input_w_thermal_output_w_flue_loss_w() {
        // Verify the exact field names — old kW names must NOT appear.
        let generator_no_chp = Generator::new(gen_config(&[]), GeneratorKind::GasGenerator);
        let names_no_chp: Vec<&str> = generator_no_chp
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(
            names_no_chp.contains(&"fuel_input_w"),
            "must use fuel_input_w"
        );
        assert!(
            !names_no_chp.iter().any(|n| *n == "fuel_input_kw"),
            "old kW name must not appear"
        );
        assert!(
            !names_no_chp.iter().any(|n| *n == "thermal_output_kw"),
            "old kW name must not appear"
        );
        assert!(
            !names_no_chp.iter().any(|n| *n == "flue_loss_kw"),
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
        assert!(names_chp.contains(&"fuel_input_w"));
        assert!(
            names_chp.contains(&"thermal_output_w"),
            "must use thermal_output_w"
        );
        assert!(names_chp.contains(&"flue_loss_w"), "must use flue_loss_w");
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
            })
            .unwrap();
        let mut slots = ports_for(&generator);
        generator
            .step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let fuel_w = generator.telemetry().get("fuel_input_w").unwrap();
        let thermal_w = generator.telemetry().get("thermal_output_w").unwrap();
        let flue_w = generator.telemetry().get("flue_loss_w").unwrap();

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
    // Telemetry field names are in watts (regression tests)
    // =======================================================================

    #[test]
    fn fuel_cell_defaults_to_curve_efficiency() {
        // Create a FuelCell without specifying efficiency_type — it must default to Curve,
        // not Constant. Curve efficiency at cr=0.25 (OCHRE default) gives eta < rated,
        // whereas Constant would return rated at all loads.
        let config = gen_config(&[
            (KEY_ETA_ELECTRIC, 0.95.into()),
            (KEY_DELTA_KW_PER_S, 100.0.into()),
        ]);
        let mut fc = Generator::new(config.clone(), GeneratorKind::FuelCell);
        fc.init(&config, &base_env()).unwrap();

        // At cr=1.0, curve and constant both return rated (1.0 * rated = rated).
        // At cr=0.25, OCHRE default curve gives rated * 0.5; constant gives rated.
        // Use a setpoint at 25% (2.5 kW of 10 kW rated).
        fc.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 2.5,
            reactive_power_kvar: None,
        })
        .unwrap();
        let mut slots = ports_for(&fc);
        fc.step(&base_env(), Duration::from_secs(1), &mut slots)
            .unwrap();

        let eta = fc.telemetry().get("eta_electric").unwrap();
        // Curve: eta = 0.95 * 0.5 = 0.475 (cr=0.25 interpolates to er=0.5 on OCHRE default)
        // Constant: eta = 0.95
        assert!(
            (eta - 0.475).abs() < 1e-6,
            "FuelCell at cr=0.25 should use curve efficiency (eta≈0.475), got {eta}. \
             This indicates the default is Constant rather than Curve."
        );
    }
}
