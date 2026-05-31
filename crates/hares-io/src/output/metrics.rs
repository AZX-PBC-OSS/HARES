//! Derived metrics and post-processing.

use std::collections::BTreeMap;

use arrow::{
    array::{Array, Float64Array},
    datatypes::{DataType, Schema},
    record_batch::RecordBatch,
};
use thiserror::Error;

use hares_types::telemetry_keys as tk;

use crate::SimulationConfig;

/// Suffix used by end-use electric power telemetry columns (OCHRE style).
const ELECTRIC_POWER_SUFFIX: &str = " Electric Power (kW)";
/// Suffix used by gas power telemetry columns (OCHRE style).
#[allow(dead_code)]
const GAS_POWER_SUFFIX: &str = " Gas Power (therms/hour)";
const TOTAL_ELECTRIC_POWER_KW: &str = "Total Electric Power (kW)";
const TOTAL_GAS_POWER_THERMS: &str = "Total Gas Power (therms/hour)";
const GRID_ELECTRIC_POWER_KW: &str = "Grid Electric Power (kW)";
const SETPOINT_DEADBAND_C: &str = "Setpoint Deadband (C)";
const DEADBAND_C: &str = "Deadband (C)";

/// Conversion factor: 1 therm = 29.3001 kWh.
const THERMS_TO_KWH: f64 = 29.3001;
const EPSILON: f64 = 1e-9;

/// Errors returned by [`MetricsCalculator::new`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MetricsError {
    #[error("time_res_secs must be greater than zero")]
    InvalidTimeResolution,
    #[error("required metrics column missing: {0}")]
    MissingRequiredColumn(String),
    #[error("column `{column}` must have Float64 type")]
    InvalidColumnType { column: String },
}

/// Annual energy metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnualEnergyKwh {
    /// Total annual electric energy (kWh).
    pub total: f64,
    /// Annual energy by end-use key.
    pub per_end_use: BTreeMap<String, f64>,
}

/// Rolling-window peak demand metrics at standard demand intervals.
#[derive(Debug, Clone, PartialEq)]
pub struct RollingPeakKw {
    /// Peak 15-minute average demand (kW).
    pub peak_15min_kw: f64,
    /// Peak 30-minute average demand (kW).
    pub peak_30min_kw: f64,
    /// Peak 60-minute average demand (kW).
    pub peak_60min_kw: f64,
}

/// Peak power metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct PeakPowerKw {
    /// Peak instantaneous power by end-use key.
    pub per_end_use: BTreeMap<String, f64>,
    /// Rolling-window peak demand metrics for grid/total power.
    pub rolling: RollingPeakKw,
}

/// Grid interaction demand metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct GridInteractionMetrics {
    /// Maximum grid import (positive draw) over the run.
    pub peak_import_kw: f64,
    /// Maximum grid export (positive feed-in) over the run.
    pub peak_export_kw: f64,
}

/// Gas energy metrics, available when a gas power column is present.
#[derive(Debug, Clone, PartialEq)]
pub struct GasEnergyMetrics {
    /// Total annual gas energy in therms.
    pub total_therms: f64,
    /// Gas energy converted to kWh (1 therm = 29.3001 kWh).
    pub total_kwh_equivalent: f64,
}

/// Envelope component loads [kWh] over the simulation period.
///
/// These represent the thermal loads imposed on the conditioned zone by each
/// envelope component. Positive = heat gain to zone, negative = heat loss.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnvelopeComponentLoadsKwh {
    /// Window transmitted + absorbed-inward solar gain.
    pub window_solar_kwh: f64,
    /// Opaque surface solar + exterior LWR.
    pub opaque_solar_lwr_kwh: f64,
    /// Interior longwave radiation exchange activity (Σ|q_i|/2, see
    /// `EnvelopeComponentGains::interior_lwr_w`). This is a gross exchange
    /// metric, not a net energy gain; summing it into a net-sensible balance
    /// would double-count energy.
    pub interior_lwr_kwh: f64,
    /// Infiltration sensible load.
    pub infiltration_kwh: f64,
    /// Ventilation sensible load.
    pub ventilation_kwh: f64,
    /// HVAC heating delivered to zone.
    pub hvac_heating_kwh: f64,
    /// HVAC cooling delivered to zone.
    pub hvac_cooling_kwh: f64,
    /// Internal gains (occupants, equipment, lighting).
    pub internal_gains_kwh: f64,
    /// Duct distribution losses to zone.
    pub duct_loss_kwh: f64,
}

/// Per-equipment efficiency metrics.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EfficiencyMetrics {
    /// HVAC heating COP: total thermal output / total electrical input.
    /// `None` if no heating occurred.
    pub hvac_heating_cop: Option<f64>,
    /// HVAC cooling COP: total cooling delivered / total electrical input.
    /// `None` if no cooling occurred.
    pub hvac_cooling_cop: Option<f64>,
    /// Water heater COP: total delivered energy / total input energy.
    /// `None` if no water heating occurred.
    pub water_heater_cop: Option<f64>,
    /// Battery round-trip efficiency: energy_out / energy_in.
    /// `None` if no battery cycling occurred.
    pub battery_round_trip_efficiency: Option<f64>,
}

/// Final simulation metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulationMetrics {
    pub annual_energy_kwh: AnnualEnergyKwh,
    pub peak_power_kw: PeakPowerKw,
    pub comfort_hours: Option<f64>,
    pub unmet_load_hours: Option<f64>,
    pub renewable_energy_fraction: Option<f64>,
    pub grid_interaction_metrics: GridInteractionMetrics,
    /// Envelope component loads. `None` if component gain columns not present.
    pub envelope_loads_kwh: Option<EnvelopeComponentLoadsKwh>,
    /// Per-equipment efficiency metrics. Always present (fields are Option).
    pub efficiency: EfficiencyMetrics,
}

/// Extended metrics returned by [`MetricsCalculator::finish`], including gas tracking.
///
/// Derefs to [`SimulationMetrics`] for backward-compatible field access.
#[derive(Debug, Clone, PartialEq)]
pub struct FullSimulationMetrics {
    /// Core simulation metrics.
    pub metrics: SimulationMetrics,
    /// Gas energy metrics. `None` when no gas power column was present.
    pub gas_energy: Option<GasEnergyMetrics>,
}

impl std::ops::Deref for FullSimulationMetrics {
    type Target = SimulationMetrics;
    fn deref(&self) -> &SimulationMetrics {
        &self.metrics
    }
}

impl FullSimulationMetrics {
    /// Gas energy metrics. `None` when no gas power column was present.
    #[must_use]
    pub fn gas_energy(&self) -> Option<&GasEnergyMetrics> {
        self.gas_energy.as_ref()
    }

    /// Combined annual energy: electric kWh + gas kWh equivalent.
    #[must_use]
    pub fn combined_annual_energy_kwh(&self) -> f64 {
        self.metrics.annual_energy_kwh.total
            + self
                .gas_energy
                .as_ref()
                .map_or(0.0, |g| g.total_kwh_equivalent)
    }
}

/// Ring buffer for rolling-window peak demand calculation.
///
/// Stores recent power readings and tracks windowed averages for
/// 15-minute, 30-minute, and 60-minute demand windows.
#[derive(Debug)]
struct RollingPeakBuffer {
    /// Circular buffer of recent total grid power readings [kW].
    buffer: Vec<f64>,
    /// Write head position.
    head: usize,
    /// Number of valid entries currently in the buffer.
    count: usize,
    /// Number of steps that fit in a 15-minute window.
    window_15min: usize,
    /// Number of steps that fit in a 30-minute window.
    window_30min: usize,
    /// Number of steps that fit in a 60-minute window.
    window_60min: usize,
    /// Tracked peak averages.
    peak_15min_kw: f64,
    peak_30min_kw: f64,
    peak_60min_kw: f64,
}

impl RollingPeakBuffer {
    fn new(timestep_h: f64) -> Self {
        // Capacity must hold at least 60 minutes of readings.
        let steps_per_hour = (1.0 / timestep_h).ceil() as usize;
        let capacity = steps_per_hour.max(1);
        let window_15min = ((0.25 / timestep_h).ceil() as usize).max(1);
        let window_30min = ((0.5 / timestep_h).ceil() as usize).max(1);
        let window_60min = capacity;
        Self {
            buffer: vec![0.0; capacity],
            head: 0,
            count: 0,
            window_15min,
            window_30min,
            window_60min,
            peak_15min_kw: 0.0,
            peak_30min_kw: 0.0,
            peak_60min_kw: 0.0,
        }
    }

    fn push(&mut self, value: f64) {
        self.buffer[self.head] = value;
        self.head = (self.head + 1) % self.buffer.len();
        self.count = (self.count + 1).min(self.buffer.len());

        let avg15 = self.window_average(self.window_15min);
        let avg30 = self.window_average(self.window_30min);
        let avg60 = self.window_average(self.window_60min);

        if avg15 > self.peak_15min_kw {
            self.peak_15min_kw = avg15;
        }
        if avg30 > self.peak_30min_kw {
            self.peak_30min_kw = avg30;
        }
        if avg60 > self.peak_60min_kw {
            self.peak_60min_kw = avg60;
        }
    }

    /// Average over the last `window` values (or fewer if not enough data yet).
    fn window_average(&self, window: usize) -> f64 {
        let n = window.min(self.count);
        if n == 0 {
            return 0.0;
        }
        let cap = self.buffer.len();
        let mut sum = 0.0;
        for i in 0..n {
            // Walk backwards from the last written position.
            let idx = (self.head + cap - 1 - i) % cap;
            sum += self.buffer[idx];
        }
        sum / n as f64
    }

    fn finish(&self) -> RollingPeakKw {
        RollingPeakKw {
            peak_15min_kw: self.peak_15min_kw.max(0.0),
            peak_30min_kw: self.peak_30min_kw.max(0.0),
            peak_60min_kw: self.peak_60min_kw.max(0.0),
        }
    }
}

/// Streaming post-processing calculator that updates from flushed Arrow batches.
#[derive(Debug)]
pub struct MetricsCalculator {
    timestep_h: f64,
    total_electric_power_idx: usize,
    total_gas_power_idx: Option<usize>,
    grid_power_idx: usize,
    pv_power_idx: Option<usize>,
    end_use_columns: Vec<(String, usize)>,
    setpoints: Option<SetpointInputs>,
    hvac_capacity_pairs: Vec<(usize, usize)>,

    // Envelope component gain column indices (optional -- verbosity >= 6)
    window_solar_w_idx: Option<usize>,
    infiltration_w_idx: Option<usize>,
    interior_lwr_w_idx: Option<usize>,
    internal_gains_w_idx: Option<usize>,
    opaque_solar_lwr_w_idx: Option<usize>,
    forced_ventilation_w_idx: Option<usize>,
    natural_ventilation_w_idx: Option<usize>,
    duct_loss_w_idx: Option<usize>,
    // HVAC thermal delivered columns (verbosity >= 4)
    hvac_heating_delivered_w_idx: Option<usize>,
    hvac_cooling_delivered_w_idx: Option<usize>,
    // HVAC electric power columns (discovered from end-use naming)
    hvac_heating_kw_idx: Option<usize>,
    hvac_cooling_kw_idx: Option<usize>,
    // Battery power column
    battery_kw_idx: Option<usize>,

    total_electric_energy_kwh: f64,
    total_gas_energy_therms: f64,
    total_consumption_kwh: f64,
    total_pv_generation_kwh_abs: f64,
    peak_import_kw: f64,
    peak_export_kw: f64,
    energy_by_end_use: BTreeMap<String, f64>,
    peak_by_end_use: BTreeMap<String, f64>,
    comfort_step_count: f64,
    unmet_step_count: f64,
    rolling_peak: RollingPeakBuffer,

    // Envelope component load accumulators [W·h → kWh at finish]
    envelope_window_solar_wh: f64,
    envelope_opaque_solar_lwr_wh: f64,
    envelope_interior_lwr_wh: f64,
    envelope_infiltration_wh: f64,
    envelope_ventilation_wh: f64,
    envelope_hvac_heating_wh: f64,
    envelope_hvac_cooling_wh: f64,
    envelope_internal_gains_wh: f64,
    envelope_duct_loss_wh: f64,
    has_envelope_columns: bool,

    // Efficiency accumulators
    hvac_heating_electric_wh: f64,
    hvac_cooling_electric_wh: f64,
    battery_energy_in_kwh: f64,
    battery_energy_out_kwh: f64,

    #[cfg(feature = "observe")]
    end_use_equipment_counts: BTreeMap<String, usize>,
}

#[derive(Debug)]
struct SetpointInputs {
    heating_setpoint_idx: usize,
    cooling_setpoint_idx: usize,
    zone_temperature_indices: Vec<usize>,
    deadband: DeadbandSource,
}

#[derive(Debug)]
enum DeadbandSource {
    Constant(f64),
    Column(usize),
}

impl MetricsCalculator {
    /// Build a streaming metrics calculator from output schema and simulation config.
    pub fn new(
        schema: &Schema,
        time_res_secs: u32,
        config: &SimulationConfig,
    ) -> Result<Self, MetricsError> {
        if time_res_secs == 0 {
            return Err(MetricsError::InvalidTimeResolution);
        }

        let total_electric_power_idx = required_float64_column(schema, TOTAL_ELECTRIC_POWER_KW)?;
        let total_gas_power_idx = optional_float64_column(schema, &[TOTAL_GAS_POWER_THERMS])?;
        let grid_power_idx = optional_float64_column(schema, &[GRID_ELECTRIC_POWER_KW])?
            .unwrap_or(total_electric_power_idx);
        let pv_power_idx = optional_float64_column(schema, &["PV Electric Power (kW)"])?;

        let end_use_columns =
            discover_end_use_columns(schema, total_electric_power_idx, grid_power_idx)?;
        let setpoints = discover_setpoint_inputs(schema, config)?;
        let hvac_capacity_pairs = discover_hvac_capacity_pairs(schema)?;

        // Discover optional envelope component gain columns (verbosity >= 6).
        let window_solar_w_idx =
            optional_float64_column(schema, &["Window Transmitted Solar Gain (W)"])?;
        let infiltration_w_idx =
            optional_float64_column(schema, &["Infiltration Heat Gain - Indoor (W)"])?;
        let interior_lwr_w_idx =
            optional_float64_column(schema, &["Interior LWR Exchange - Indoor (W)"])?;
        let internal_gains_w_idx =
            optional_float64_column(schema, &["Internal Heat Gain - Indoor (W)"])?;
        let opaque_solar_lwr_w_idx =
            optional_float64_column(schema, &["Opaque Surface Heat Gain - Indoor (W)"])?;
        let forced_ventilation_w_idx =
            optional_float64_column(schema, &["Forced Ventilation Heat Gain - Indoor (W)"])?;
        let natural_ventilation_w_idx =
            optional_float64_column(schema, &["Natural Ventilation Heat Gain - Indoor (W)"])?;
        let duct_loss_w_idx =
            optional_float64_column(schema, &["Duct Loss Heat Gain - Indoor (W)"])?;

        // Discover HVAC thermal delivered columns (verbosity >= 4).
        let hvac_heating_delivered_w_idx =
            optional_float64_column(schema, &["HVAC Heating Delivered (W)"])?;
        let hvac_cooling_delivered_w_idx =
            optional_float64_column(schema, &["HVAC Cooling Delivered (W)"])?;

        // Discover HVAC electric power and battery columns from end-use naming.
        let hvac_heating_kw_idx =
            optional_float64_column(schema, &["HVAC Heating Electric Power (kW)"])?;
        let hvac_cooling_kw_idx =
            optional_float64_column(schema, &["HVAC Cooling Electric Power (kW)"])?;
        let battery_kw_idx = optional_float64_column(schema, &["Battery Electric Power (kW)"])?;

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            // Only warn about missing HVAC aggregate columns when the schema
            // contains per-equipment columns for that EndUse category.
            // Without this gate, the check fires at every verbosity-0 run
            // regardless of whether HVAC equipment is present.
            let has_hvac_heating_equipment =
                schema_has_equipment_for(schema, &hares_types::EndUse::HVAC_HEATING);
            let has_hvac_cooling_equipment =
                schema_has_equipment_for(schema, &hares_types::EndUse::HVAC_COOLING);

            if hvac_heating_kw_idx.is_none() && has_hvac_heating_equipment {
                tracing::warn!(
                    "HVAC Heating aggregate column ('{}') not found in schema — \
                     hvac_heating_electric_wh will be zero",
                    "HVAC Heating Electric Power (kW)"
                );
            }
            if hvac_cooling_kw_idx.is_none() && has_hvac_cooling_equipment {
                tracing::warn!(
                    "HVAC Cooling aggregate column ('{}') not found in schema — \
                     hvac_cooling_electric_wh will be zero",
                    "HVAC Cooling Electric Power (kW)"
                );
            }
        }

        let timestep_h = f64::from(time_res_secs) / 3600.0;
        let energy_by_end_use = end_use_columns
            .iter()
            .map(|(name, _)| (name.clone(), 0.0))
            .collect();
        let peak_by_end_use = end_use_columns
            .iter()
            .map(|(name, _)| (name.clone(), f64::NEG_INFINITY))
            .collect();
        let rolling_peak = RollingPeakBuffer::new(timestep_h);

        Ok(Self {
            timestep_h,
            total_electric_power_idx,
            total_gas_power_idx,
            grid_power_idx,
            pv_power_idx,
            end_use_columns,
            setpoints,
            hvac_capacity_pairs,
            window_solar_w_idx,
            infiltration_w_idx,
            interior_lwr_w_idx,
            internal_gains_w_idx,
            opaque_solar_lwr_w_idx,
            forced_ventilation_w_idx,
            natural_ventilation_w_idx,
            duct_loss_w_idx,
            hvac_heating_delivered_w_idx,
            hvac_cooling_delivered_w_idx,
            hvac_heating_kw_idx,
            hvac_cooling_kw_idx,
            battery_kw_idx,
            total_electric_energy_kwh: 0.0,
            total_gas_energy_therms: 0.0,
            total_consumption_kwh: 0.0,
            total_pv_generation_kwh_abs: 0.0,
            peak_import_kw: 0.0,
            peak_export_kw: 0.0,
            energy_by_end_use,
            peak_by_end_use,
            comfort_step_count: 0.0,
            unmet_step_count: 0.0,
            rolling_peak,
            envelope_window_solar_wh: 0.0,
            envelope_opaque_solar_lwr_wh: 0.0,
            envelope_interior_lwr_wh: 0.0,
            envelope_infiltration_wh: 0.0,
            envelope_ventilation_wh: 0.0,
            envelope_hvac_heating_wh: 0.0,
            envelope_hvac_cooling_wh: 0.0,
            envelope_internal_gains_wh: 0.0,
            envelope_duct_loss_wh: 0.0,
            has_envelope_columns: window_solar_w_idx.is_some()
                || infiltration_w_idx.is_some()
                || interior_lwr_w_idx.is_some()
                || internal_gains_w_idx.is_some()
                || opaque_solar_lwr_w_idx.is_some()
                || forced_ventilation_w_idx.is_some()
                || natural_ventilation_w_idx.is_some()
                || duct_loss_w_idx.is_some()
                || hvac_heating_delivered_w_idx.is_some()
                || hvac_cooling_delivered_w_idx.is_some(),
            hvac_heating_electric_wh: 0.0,
            hvac_cooling_electric_wh: 0.0,
            battery_energy_in_kwh: 0.0,
            battery_energy_out_kwh: 0.0,
            #[cfg(feature = "observe")]
            end_use_equipment_counts: compute_equipment_counts_from_schema(schema),
        })
    }

    /// Set end-use equipment counts for observer diagnostics.
    #[cfg(feature = "observe")]
    pub fn set_end_use_equipment_counts(&mut self, counts: BTreeMap<String, usize>) {
        self.end_use_equipment_counts = counts;
    }

    /// Update metrics from a single flushed batch.
    pub fn accumulate(&mut self, batch: &RecordBatch) {
        if batch.num_rows() == 0 {
            return;
        }

        let total_arr = as_f64_array(batch, self.total_electric_power_idx);
        let gas_arr = self.total_gas_power_idx.map(|idx| as_f64_array(batch, idx));
        let grid_arr = as_f64_array(batch, self.grid_power_idx);
        let pv_arr = self.pv_power_idx.map(|idx| as_f64_array(batch, idx));

        let mut end_use_arrays = Vec::with_capacity(self.end_use_columns.len());
        for (_, idx) in &self.end_use_columns {
            end_use_arrays.push(as_f64_array(batch, *idx));
        }

        let setpoint_arrays = self.setpoints.as_ref().map(|inputs| {
            let deadband_arr = match inputs.deadband {
                DeadbandSource::Constant(_) => None,
                DeadbandSource::Column(idx) => Some(as_f64_array(batch, idx)),
            };

            (
                as_f64_array(batch, inputs.heating_setpoint_idx),
                as_f64_array(batch, inputs.cooling_setpoint_idx),
                inputs
                    .zone_temperature_indices
                    .iter()
                    .map(|idx| as_f64_array(batch, *idx))
                    .collect::<Vec<_>>(),
                deadband_arr,
            )
        });

        let hvac_pairs = self
            .hvac_capacity_pairs
            .iter()
            .map(|(output_idx, capacity_idx)| {
                (
                    as_f64_array(batch, *output_idx),
                    as_f64_array(batch, *capacity_idx),
                )
            })
            .collect::<Vec<_>>();

        for row in 0..batch.num_rows() {
            if let Some(total_kw) = value_at(total_arr, row) {
                self.total_electric_energy_kwh += total_kw * self.timestep_h;
                if total_kw > 0.0 {
                    self.total_consumption_kwh += total_kw * self.timestep_h;
                }
            }

            if let Some(gas_therms_per_h) = gas_arr.and_then(|arr| value_at(arr, row)) {
                self.total_gas_energy_therms += gas_therms_per_h * self.timestep_h;
            }

            if let Some(grid_kw) = value_at(grid_arr, row) {
                if grid_kw > self.peak_import_kw {
                    self.peak_import_kw = grid_kw;
                }
                let export_kw = (-grid_kw).max(0.0);
                if export_kw > self.peak_export_kw {
                    self.peak_export_kw = export_kw;
                }
                self.rolling_peak.push(grid_kw);
            }

            if let Some(pv) = pv_arr.and_then(|arr| value_at(arr, row))
                && pv < 0.0
            {
                self.total_pv_generation_kwh_abs += pv.abs() * self.timestep_h;
            }

            for ((name, _), array) in self.end_use_columns.iter().zip(&end_use_arrays) {
                if let Some(value_kw) = value_at(array, row) {
                    if let Some(sum_kwh) = self.energy_by_end_use.get_mut(name) {
                        *sum_kwh += value_kw * self.timestep_h;
                    }
                    if let Some(peak_kw) = self.peak_by_end_use.get_mut(name)
                        && value_kw > *peak_kw
                    {
                        *peak_kw = value_kw;
                    }
                }
            }

            if let (Some(inputs), Some((heating, cooling, zone_temps, deadband_arr))) =
                (self.setpoints.as_ref(), setpoint_arrays.as_ref())
            {
                let Some(heating_c) = value_at(heating, row) else {
                    continue;
                };
                let Some(cooling_c) = value_at(cooling, row) else {
                    continue;
                };
                let deadband_c = match (&inputs.deadband, deadband_arr) {
                    (DeadbandSource::Constant(value), _) => *value,
                    (DeadbandSource::Column(_), Some(arr)) => value_at(arr, row).unwrap_or(0.0),
                    (DeadbandSource::Column(_), None) => 0.0,
                };

                if !deadband_c.is_finite() || deadband_c < 0.0 {
                    continue;
                }

                let lower_bound_c = heating_c - deadband_c / 2.0;
                let upper_bound_c = cooling_c + deadband_c / 2.0;
                let mut all_inside = true;
                let mut any_outside = false;
                for zone_arr in zone_temps {
                    let Some(zone_c) = value_at(zone_arr, row) else {
                        all_inside = false;
                        continue;
                    };
                    if zone_c < lower_bound_c || zone_c > upper_bound_c {
                        all_inside = false;
                        any_outside = true;
                    }
                }
                if all_inside {
                    self.comfort_step_count += 1.0;
                }

                if any_outside && is_hvac_at_capacity(&hvac_pairs, row) {
                    self.unmet_step_count += 1.0;
                }
            }

            // Envelope component loads [W → Wh via timestep_h].
            if let Some(idx) = self.window_solar_w_idx {
                if let Some(v) = value_at(as_f64_array(batch, idx), row) {
                    self.envelope_window_solar_wh += v * self.timestep_h;
                }
            }
            if let Some(idx) = self.infiltration_w_idx {
                if let Some(v) = value_at(as_f64_array(batch, idx), row) {
                    self.envelope_infiltration_wh += v * self.timestep_h;
                }
            }
            if let Some(idx) = self.interior_lwr_w_idx {
                if let Some(v) = value_at(as_f64_array(batch, idx), row) {
                    self.envelope_interior_lwr_wh += v * self.timestep_h;
                }
            }
            if let Some(idx) = self.internal_gains_w_idx {
                if let Some(v) = value_at(as_f64_array(batch, idx), row) {
                    self.envelope_internal_gains_wh += v * self.timestep_h;
                }
            }
            if let Some(idx) = self.opaque_solar_lwr_w_idx {
                if let Some(v) = value_at(as_f64_array(batch, idx), row) {
                    self.envelope_opaque_solar_lwr_wh += v * self.timestep_h;
                }
            }
            if let Some(idx) = self.forced_ventilation_w_idx {
                if let Some(v) = value_at(as_f64_array(batch, idx), row) {
                    self.envelope_ventilation_wh += v * self.timestep_h;
                }
            }
            if let Some(idx) = self.natural_ventilation_w_idx {
                if let Some(v) = value_at(as_f64_array(batch, idx), row) {
                    self.envelope_ventilation_wh += v * self.timestep_h;
                }
            }
            if let Some(idx) = self.duct_loss_w_idx {
                if let Some(v) = value_at(as_f64_array(batch, idx), row) {
                    self.envelope_duct_loss_wh += v * self.timestep_h;
                }
            }

            // HVAC thermal delivered accumulation for COP and envelope loads.
            if let Some(idx) = self.hvac_heating_delivered_w_idx {
                if let Some(v) = value_at(as_f64_array(batch, idx), row) {
                    self.envelope_hvac_heating_wh += v * self.timestep_h;
                }
            }
            if let Some(idx) = self.hvac_cooling_delivered_w_idx {
                if let Some(v) = value_at(as_f64_array(batch, idx), row) {
                    self.envelope_hvac_cooling_wh += v * self.timestep_h;
                }
            }

            // HVAC electric power accumulation for COP.
            if let Some(idx) = self.hvac_heating_kw_idx {
                if let Some(kw) = value_at(as_f64_array(batch, idx), row) {
                    if kw > 0.0 {
                        self.hvac_heating_electric_wh += kw * 1000.0 * self.timestep_h;
                    }
                }
            }
            if let Some(idx) = self.hvac_cooling_kw_idx {
                if let Some(kw) = value_at(as_f64_array(batch, idx), row) {
                    if kw > 0.0 {
                        self.hvac_cooling_electric_wh += kw * 1000.0 * self.timestep_h;
                    }
                }
            }

            // Battery energy tracking for round-trip efficiency.
            if let Some(idx) = self.battery_kw_idx {
                if let Some(kw) = value_at(as_f64_array(batch, idx), row) {
                    if kw > 0.0 {
                        self.battery_energy_in_kwh += kw * self.timestep_h;
                    } else if kw < 0.0 {
                        self.battery_energy_out_kwh += kw.abs() * self.timestep_h;
                    }
                }
            }

            #[cfg(feature = "observe")]
            if row == batch.num_rows() - 1 {
                let mut parts: Vec<String> = Vec::new();
                for (key, val) in &self.energy_by_end_use {
                    parts.push(format!("{key}={val:.3}kWh"));
                }
                for (key, count) in &self.end_use_equipment_counts {
                    parts.push(format!("n_{key}={count}"));
                }
                parts.push(format!(
                    "hvac_heating_wh={:.1}",
                    self.hvac_heating_electric_wh
                ));
                parts.push(format!(
                    "hvac_cooling_wh={:.1}",
                    self.hvac_cooling_electric_wh
                ));
                tracing::info!(
                    end_use_diagnostics = %parts.join(", "),
                    "end-use metrics at batch boundary"
                );
            }
        }
    }

    /// Finalize and return all derived metrics.
    #[must_use]
    pub fn finish(self) -> FullSimulationMetrics {
        let mut peak_by_end_use = self.peak_by_end_use;
        for value in peak_by_end_use.values_mut() {
            if *value == f64::NEG_INFINITY {
                *value = 0.0;
            }
        }

        let renewable_energy_fraction = self.pv_power_idx.map(|_| {
            if self.total_consumption_kwh <= 0.0 {
                0.0
            } else {
                (self.total_pv_generation_kwh_abs / self.total_consumption_kwh).clamp(0.0, 1.0)
            }
        });

        let gas_energy = self.total_gas_power_idx.map(|_| {
            let therms = self.total_gas_energy_therms;
            GasEnergyMetrics {
                total_therms: therms,
                total_kwh_equivalent: therms * THERMS_TO_KWH,
            }
        });

        FullSimulationMetrics {
            metrics: SimulationMetrics {
                annual_energy_kwh: AnnualEnergyKwh {
                    total: self.total_electric_energy_kwh,
                    per_end_use: self.energy_by_end_use,
                },
                peak_power_kw: PeakPowerKw {
                    per_end_use: peak_by_end_use,
                    rolling: self.rolling_peak.finish(),
                },
                comfort_hours: self
                    .setpoints
                    .as_ref()
                    .map(|_| self.comfort_step_count * self.timestep_h),
                unmet_load_hours: self
                    .setpoints
                    .as_ref()
                    .map(|_| self.unmet_step_count * self.timestep_h),
                renewable_energy_fraction,
                grid_interaction_metrics: GridInteractionMetrics {
                    peak_import_kw: self.peak_import_kw.max(0.0),
                    peak_export_kw: self.peak_export_kw.max(0.0),
                },
                envelope_loads_kwh: if self.has_envelope_columns {
                    Some(EnvelopeComponentLoadsKwh {
                        window_solar_kwh: self.envelope_window_solar_wh / 1000.0,
                        opaque_solar_lwr_kwh: self.envelope_opaque_solar_lwr_wh / 1000.0,
                        interior_lwr_kwh: self.envelope_interior_lwr_wh / 1000.0,
                        infiltration_kwh: self.envelope_infiltration_wh / 1000.0,
                        ventilation_kwh: self.envelope_ventilation_wh / 1000.0,
                        hvac_heating_kwh: self.envelope_hvac_heating_wh / 1000.0,
                        hvac_cooling_kwh: self.envelope_hvac_cooling_wh / 1000.0,
                        internal_gains_kwh: self.envelope_internal_gains_wh / 1000.0,
                        duct_loss_kwh: self.envelope_duct_loss_wh / 1000.0,
                    })
                } else {
                    None
                },
                efficiency: EfficiencyMetrics {
                    hvac_heating_cop: if self.hvac_heating_electric_wh > EPSILON {
                        // Thermal output from envelope gains; electric input from telemetry column.
                        let thermal = self.envelope_hvac_heating_wh.max(0.0);
                        Some(thermal / self.hvac_heating_electric_wh)
                    } else {
                        None
                    },
                    hvac_cooling_cop: if self.hvac_cooling_electric_wh > EPSILON {
                        let thermal = self.envelope_hvac_cooling_wh.abs();
                        Some(thermal / self.hvac_cooling_electric_wh)
                    } else {
                        None
                    },
                    water_heater_cop: None, // requires WH-specific telemetry columns
                    battery_round_trip_efficiency: if self.battery_energy_in_kwh > EPSILON {
                        Some(self.battery_energy_out_kwh / self.battery_energy_in_kwh)
                    } else {
                        None
                    },
                },
            },
            gas_energy,
        }
    }
}

fn discover_end_use_columns(
    schema: &Schema,
    total_electric_power_idx: usize,
    grid_power_idx: usize,
) -> Result<Vec<(String, usize)>, MetricsError> {
    let mut columns = Vec::new();
    for (idx, field) in schema.fields().iter().enumerate() {
        if idx == total_electric_power_idx || idx == grid_power_idx {
            continue;
        }
        let name = field.name().as_str();
        // Match OCHRE-style per-end-use aggregate columns:
        // "{EndUse Display Name} Electric Power (kW)"
        if name.ends_with(ELECTRIC_POWER_SUFFIX) {
            let prefix = name
                .strip_suffix(ELECTRIC_POWER_SUFFIX)
                .unwrap_or(name)
                .trim();
            // Only include columns whose prefix matches a known EndUse display
            // name — this filters out per-equipment columns (e.g. "ASHP Heater
            // Electric Power (kW)") and keeps only aggregate columns
            // (e.g. "HVAC Heating Electric Power (kW)").
            if let Some(end_use_key) = crate::output::columns::display_name_to_end_use_key(prefix) {
                ensure_float64(schema, idx)?;
                columns.push((end_use_key, idx));
            }
        }
    }
    Ok(columns)
}

/// Returns true if the schema contains at least one per-equipment electric
/// power column that maps to the given `EndUse` category.
///
/// Used by invariant checks to gate warnings about missing aggregate columns
/// on the actual presence of equipment for that end-use category.
fn schema_has_equipment_for(schema: &Schema, target: &hares_types::EndUse) -> bool {
    for field in schema.fields() {
        let name = field.name().as_str();
        if name.ends_with(ELECTRIC_POWER_SUFFIX) {
            let prefix = name
                .strip_suffix(ELECTRIC_POWER_SUFFIX)
                .unwrap_or(name)
                .trim();
            // Strip instance qualifier suffix (" #N") for multi-instance
            // equipment columns (e.g. "ASHP Heater #1" -> "ASHP Heater").
            let base = match prefix.rfind(" #") {
                Some(pos) => &prefix[..pos],
                None => prefix,
            };
            if crate::output::columns::equipment_name_to_end_use(base) == *target {
                return true;
            }
        }
    }
    false
}

/// Derives per-end-use equipment counts from the output schema.
///
/// Counts each per-equipment electric power column whose name maps to a
/// recognised EndUse category, skipping aggregate columns (whose prefix is
/// an EndUse display name rather than an equipment instance name).
#[cfg(feature = "observe")]
fn compute_equipment_counts_from_schema(schema: &Schema) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for field in schema.fields() {
        let name = field.name().as_str();
        if name.ends_with(ELECTRIC_POWER_SUFFIX) {
            let prefix = name
                .strip_suffix(ELECTRIC_POWER_SUFFIX)
                .unwrap_or(name)
                .trim();
            let base = match prefix.rfind(" #") {
                Some(pos) => &prefix[..pos],
                None => prefix,
            };
            let end_use = crate::output::columns::equipment_name_to_end_use(base);
            // Only count columns that represent real equipment instances:
            // aggregate columns (e.g. "HVAC Heating") do not match
            // equipment_name_to_end_use and return EndUse::OTHER.
            if end_use != hares_types::EndUse::OTHER {
                let display = crate::output::columns::end_use_display_name(&end_use);
                if let Some(key) = crate::output::columns::display_name_to_end_use_key(display) {
                    *counts.entry(key).or_insert(0) += 1;
                }
            }
        }
    }
    counts
}

fn discover_setpoint_inputs(
    schema: &Schema,
    config: &SimulationConfig,
) -> Result<Option<SetpointInputs>, MetricsError> {
    let heating_setpoint_idx = optional_float64_column(schema, &["HVAC Heating Setpoint (C)"])?;
    let cooling_setpoint_idx = optional_float64_column(schema, &["HVAC Cooling Setpoint (C)"])?;

    let (Some(heating_setpoint_idx), Some(cooling_setpoint_idx)) =
        (heating_setpoint_idx, cooling_setpoint_idx)
    else {
        return Ok(None);
    };

    let zone_temperature_indices = discover_conditioned_zone_temperatures(schema)?;
    if zone_temperature_indices.is_empty() {
        return Ok(None);
    }

    let deadband = if let Some(deadband_c) = config.setpoint_deadband_c {
        DeadbandSource::Constant(deadband_c)
    } else if let Some(idx) = optional_float64_column(schema, &[SETPOINT_DEADBAND_C, DEADBAND_C])? {
        DeadbandSource::Column(idx)
    } else {
        return Ok(None);
    };

    Ok(Some(SetpointInputs {
        heating_setpoint_idx,
        cooling_setpoint_idx,
        zone_temperature_indices,
        deadband,
    }))
}

fn discover_conditioned_zone_temperatures(schema: &Schema) -> Result<Vec<usize>, MetricsError> {
    // OCHRE convention: "Temperature - Indoor (C)", "Temperature - {zone} (C)"
    let mut conditioned = Vec::new();
    for (idx, field) in schema.fields().iter().enumerate() {
        let name = field.name().as_str();
        if name == "Temperature - Indoor (C)" {
            ensure_float64(schema, idx)?;
            conditioned.push(idx);
        }
    }
    if !conditioned.is_empty() {
        return Ok(conditioned);
    }

    // Fallback: any "Temperature - {zone} (C)" column.
    let mut generic = Vec::new();
    for (idx, field) in schema.fields().iter().enumerate() {
        let name = field.name().as_str();
        if name.starts_with("Temperature - ") && name.ends_with(" (C)") {
            ensure_float64(schema, idx)?;
            generic.push(idx);
        }
    }
    Ok(generic)
}

fn discover_hvac_capacity_pairs(schema: &Schema) -> Result<Vec<(usize, usize)>, MetricsError> {
    let mut pairs = Vec::new();
    // Try heating pair.
    if let (Some(output_idx), Some(capacity_idx)) = (
        optional_float64_column(schema, &["HVAC Heating Delivered (W)"])?,
        optional_float64_column(schema, &[tk::HVAC_HEATING_CAPACITY_W])?,
    ) {
        pairs.push((output_idx, capacity_idx));
    }
    // Try cooling pair.
    if let (Some(output_idx), Some(capacity_idx)) = (
        optional_float64_column(schema, &["HVAC Cooling Delivered (W)"])?,
        optional_float64_column(schema, &[tk::HVAC_COOLING_CAPACITY_W])?,
    ) {
        pairs.push((output_idx, capacity_idx));
    }
    Ok(pairs)
}

/// Relative tolerance for HVAC capacity comparison. Avoids false negatives
/// at high wattages where the absolute difference may exceed a tiny epsilon.
/// The `max(1.0)` denominator clamp means this transitions to a near-absolute
/// tolerance (~1e-6 W) below 1 W capacity, which is acceptable since sub-1W
/// HVAC capacities are unphysical.
const HVAC_CAPACITY_REL_TOL: f64 = 1e-6;

fn is_hvac_at_capacity(pairs: &[(&Float64Array, &Float64Array)], row: usize) -> bool {
    // Column units are watts (e.g. "HVAC Heating Delivered (W)").
    pairs.iter().any(
        |(output, capacity)| match (value_at(output, row), value_at(capacity, row)) {
            (Some(output_w), Some(capacity_w)) if capacity_w.abs() > EPSILON => {
                let diff = (output_w.abs() - capacity_w.abs()).abs();
                diff / capacity_w.abs().max(1.0) < HVAC_CAPACITY_REL_TOL
            }
            _ => false,
        },
    )
}

fn optional_float64_column(
    schema: &Schema,
    candidates: &[&str],
) -> Result<Option<usize>, MetricsError> {
    for name in candidates {
        if let Some((idx, _)) = schema.column_with_name(name) {
            ensure_float64(schema, idx)?;
            return Ok(Some(idx));
        }
    }
    Ok(None)
}

fn required_float64_column(schema: &Schema, name: &str) -> Result<usize, MetricsError> {
    let Some((idx, _)) = schema.column_with_name(name) else {
        return Err(MetricsError::MissingRequiredColumn(name.to_owned()));
    };
    ensure_float64(schema, idx)?;
    Ok(idx)
}

fn ensure_float64(schema: &Schema, idx: usize) -> Result<(), MetricsError> {
    let field = schema.field(idx);
    if !matches!(field.data_type(), DataType::Float64) {
        return Err(MetricsError::InvalidColumnType {
            column: field.name().to_owned(),
        });
    }
    Ok(())
}

fn as_f64_array(batch: &RecordBatch, index: usize) -> &Float64Array {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("schema type is validated at construction")
}

fn value_at(arr: &Float64Array, row: usize) -> Option<f64> {
    if arr.is_null(row) {
        None
    } else {
        Some(arr.value(row))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::ArrayRef,
        datatypes::{Field, Schema},
    };
    use chrono::{Duration, FixedOffset, TimeZone};
    use std::sync::Arc;

    fn test_config(deadband: Option<f64>) -> SimulationConfig {
        SimulationConfig {
            start_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .unwrap(),
            duration: Duration::hours(1),
            time_res: Duration::hours(1),
            output_verbosity: 0,
            output_path: None,
            write_output: true,
            output_format: crate::OutputFormat::Csv,
            output_chunk_size: 16,
            master_seed: 0,
            setpoint_deadband_c: deadband,
            civil_timezone: None,
        }
    }

    fn build_batch(columns: Vec<(&str, Vec<f64>)>) -> RecordBatch {
        let fields = columns
            .iter()
            .map(|(name, _)| Field::new(*name, DataType::Float64, false))
            .collect::<Vec<_>>();
        let arrays: Vec<ArrayRef> = columns
            .into_iter()
            .map(|(_, values)| Arc::new(Float64Array::from(values)) as ArrayRef)
            .collect();
        let schema = Arc::new(Schema::new(fields));
        RecordBatch::try_new(schema, arrays).expect("record batch")
    }

    fn schema_from_columns(names: &[&str]) -> Schema {
        Schema::new(
            names
                .iter()
                .map(|name| Field::new(*name, DataType::Float64, false))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn annual_energy_total_matches_8760_constant_load() {
        let schema =
            schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "HVAC Heating Electric Power (kW)"]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");

        let rows = 8_760;
        let batch = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0; rows]),
            ("HVAC Heating Electric Power (kW)", vec![1.0; rows]),
        ]);
        calc.accumulate(&batch);
        let metrics = calc.finish();

        assert!((metrics.annual_energy_kwh.total - 8_760.0).abs() < 1e-9);
    }

    #[test]
    fn peak_power_uses_max_across_batches() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "EV Electric Power (kW)"]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");

        let b1 = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![2.0, 4.0]),
            ("EV Electric Power (kW)", vec![1.0, 3.5]),
        ]);
        let b2 = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![3.0, 5.0]),
            ("EV Electric Power (kW)", vec![2.5, 6.2]),
        ]);
        calc.accumulate(&b1);
        calc.accumulate(&b2);
        let metrics = calc.finish();

        assert_eq!(metrics.peak_power_kw.per_end_use["ev"], 6.2);
    }

    #[test]
    fn comfort_hours_equals_duration_when_all_inside_deadband() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Temperature - Indoor (C)",
            "HVAC Heating Setpoint (C)",
            "HVAC Cooling Setpoint (C)",
            "HVAC Heating Delivered (W)",
            tk::HVAC_HEATING_CAPACITY_W,
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(Some(1.0))).expect("new");

        let batch = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0, 1.0]),
            ("Temperature - Indoor (C)", vec![21.0, 21.4, 22.0]),
            ("HVAC Heating Setpoint (C)", vec![21.0, 21.0, 21.0]),
            ("HVAC Cooling Setpoint (C)", vec![22.0, 22.0, 22.0]),
            ("HVAC Heating Delivered (W)", vec![0.0, 0.0, 0.0]),
            (tk::HVAC_HEATING_CAPACITY_W, vec![4.0, 4.0, 4.0]),
        ]);
        calc.accumulate(&batch);
        let metrics = calc.finish();

        assert_eq!(metrics.comfort_hours, Some(3.0));
        assert_eq!(metrics.unmet_load_hours, Some(0.0));
    }

    #[test]
    fn verbosity_zero_schema_returns_none_for_gated_metrics() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        calc.accumulate(&build_batch(vec![(
            TOTAL_ELECTRIC_POWER_KW,
            vec![1.0, 2.0],
        )]));
        let metrics = calc.finish();

        assert_eq!(metrics.comfort_hours, None);
        assert_eq!(metrics.unmet_load_hours, None);
    }

    #[test]
    fn renewable_fraction_uses_abs_for_negative_pv() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "PV Electric Power (kW)"]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![4.0, 4.0]),
            ("PV Electric Power (kW)", vec![-2.0, -1.0]),
        ]));
        let metrics = calc.finish();

        assert_eq!(metrics.renewable_energy_fraction, Some(0.375));
    }

    #[test]
    fn renewable_fraction_none_without_pv_column() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        let metrics = calc.finish();
        assert_eq!(metrics.renewable_energy_fraction, None);
    }

    #[test]
    fn peak_export_is_zero_when_grid_never_negative() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, GRID_ELECTRIC_POWER_KW]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 2.0, 3.0]),
            (GRID_ELECTRIC_POWER_KW, vec![0.0, 2.0, 1.5]),
        ]));
        let metrics = calc.finish();
        assert_eq!(metrics.grid_interaction_metrics.peak_export_kw, 0.0);
        assert_eq!(metrics.grid_interaction_metrics.peak_import_kw, 2.0);
    }

    #[test]
    fn unmet_load_hours_count_rows_at_capacity_and_outside_setpoint() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Temperature - Indoor (C)",
            "HVAC Heating Setpoint (C)",
            "HVAC Cooling Setpoint (C)",
            "Setpoint Deadband (C)",
            "HVAC Heating Delivered (W)",
            tk::HVAC_HEATING_CAPACITY_W,
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");

        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0, 1.0]),
            ("Temperature - Indoor (C)", vec![24.0, 22.0, 24.0]),
            ("HVAC Heating Setpoint (C)", vec![21.0, 21.0, 21.0]),
            ("HVAC Cooling Setpoint (C)", vec![22.0, 22.0, 22.0]),
            ("Setpoint Deadband (C)", vec![1.0, 1.0, 1.0]),
            ("HVAC Heating Delivered (W)", vec![3.0, 2.0, 3.0]),
            (tk::HVAC_HEATING_CAPACITY_W, vec![3.0, 3.0, 3.0]),
        ]));
        let metrics = calc.finish();

        assert_eq!(metrics.unmet_load_hours, Some(2.0));
    }

    #[test]
    fn constructor_requires_total_electric_power_column() {
        let schema = schema_from_columns(&["HVAC Heating Electric Power (kW)"]);
        let err = MetricsCalculator::new(&schema, 3600, &test_config(None)).unwrap_err();
        assert_eq!(
            err,
            MetricsError::MissingRequiredColumn(TOTAL_ELECTRIC_POWER_KW.to_string())
        );
    }

    #[test]
    fn gas_energy_tracked_when_gas_column_present() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, TOTAL_GAS_POWER_THERMS]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0]),
            (TOTAL_GAS_POWER_THERMS, vec![2.0, 3.0]),
        ]));
        let metrics = calc.finish();

        let gas = metrics
            .gas_energy
            .as_ref()
            .expect("gas energy should be present");
        assert!((gas.total_therms - 5.0).abs() < 1e-9);
        assert!((metrics.annual_energy_kwh.total - 2.0).abs() < 1e-9);
        let expected_combined = 2.0 + 5.0 * 29.3001;
        assert!((metrics.combined_annual_energy_kwh() - expected_combined).abs() < 1e-6);
    }

    #[test]
    fn gas_energy_none_when_no_gas_column() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        let metrics = calc.finish();
        assert!(metrics.gas_energy.is_none());
        assert!((metrics.combined_annual_energy_kwh() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn schema_from_build_schema_works_with_metrics_calculator() {
        // Integration test: build_schema output must be accepted by MetricsCalculator.
        let specs = vec![crate::hpxml::EquipmentSpec {
            instance_name: None,
            name: "ASHP Heater".to_string(),
            fuel_type: hares_types::FuelType::Electric,
            parameters: serde_json::Map::new(),
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }];
        let schema = crate::output::build_schema(&specs, 1, &[]);
        let result = MetricsCalculator::new(&schema, 3600, &test_config(None));
        assert!(
            result.is_ok(),
            "MetricsCalculator should accept build_schema output: {:?}",
            result.err()
        );
    }

    #[test]
    fn electric_and_gas_metrics_both_populated() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            TOTAL_GAS_POWER_THERMS,
            "Temperature - Indoor (C)",
            "HVAC Heating Setpoint (C)",
            "HVAC Cooling Setpoint (C)",
            "HVAC Heating Delivered (W)",
            tk::HVAC_HEATING_CAPACITY_W,
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(Some(1.0))).expect("new");

        let rows = 24;
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![2.0; rows]),
            (TOTAL_GAS_POWER_THERMS, vec![0.5; rows]),
            ("Temperature - Indoor (C)", vec![21.5; rows]),
            ("HVAC Heating Setpoint (C)", vec![21.0; rows]),
            ("HVAC Cooling Setpoint (C)", vec![23.0; rows]),
            ("HVAC Heating Delivered (W)", vec![1.0; rows]),
            (tk::HVAC_HEATING_CAPACITY_W, vec![5.0; rows]),
        ]));
        let metrics = calc.finish();

        assert!(
            metrics.annual_energy_kwh.total > 0.0,
            "annual electric energy must be > 0"
        );
        assert!(
            metrics.comfort_hours.unwrap_or(0.0) > 0.0,
            "comfort hours must be > 0"
        );
        assert!(
            metrics.grid_interaction_metrics.peak_import_kw > 0.0,
            "peak demand must be > 0"
        );
        let gas = metrics.gas_energy.expect("gas energy must be present");
        assert!(gas.total_therms > 0.0, "gas therms must be > 0");
        assert!(gas.total_kwh_equivalent > 0.0, "gas kWh must be > 0");
    }

    #[test]
    fn hvac_at_capacity_uses_relative_tolerance() {
        // At high wattages, absolute 1e-9 tolerance would produce false negatives.
        // The relative tolerance should correctly detect capacity saturation.
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Temperature - Indoor (C)",
            "HVAC Heating Setpoint (C)",
            "HVAC Cooling Setpoint (C)",
            "HVAC Heating Delivered (W)",
            tk::HVAC_HEATING_CAPACITY_W,
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(Some(1.0))).expect("new");

        // Row with output nearly equal to capacity at high wattage (50,000 W = 50 kW).
        // Difference = 0.00005 W → relative = 0.00005 / 50000 = 1e-9, well within 1e-6.
        let capacity = 50_000.0;
        let output = capacity - 0.00005;
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0]),
            ("Temperature - Indoor (C)", vec![25.0]), // outside cooling setpoint
            ("HVAC Heating Setpoint (C)", vec![21.0]),
            ("HVAC Cooling Setpoint (C)", vec![22.0]),
            ("HVAC Heating Delivered (W)", vec![output]),
            (tk::HVAC_HEATING_CAPACITY_W, vec![capacity]),
        ]));
        let metrics = calc.finish();

        assert_eq!(
            metrics.unmet_load_hours,
            Some(1.0),
            "should detect capacity saturation at high wattage"
        );
    }

    #[test]
    fn hvac_heating_cop_computed_from_delivered_and_electric() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating Electric Power (kW)",
            "HVAC Heating Delivered (W)",
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");

        // 10 kW thermal delivery, 2 kW electric → COP = 10000 Wh / 2000 Wh = 5.0
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![2.0, 2.0]),
            ("HVAC Heating Electric Power (kW)", vec![2.0, 2.0]),
            ("HVAC Heating Delivered (W)", vec![10_000.0, 10_000.0]),
        ]));
        let metrics = calc.finish();

        let cop = metrics
            .metrics
            .efficiency
            .hvac_heating_cop
            .expect("COP should be present");
        assert!(
            (cop - 5.0).abs() < 1e-9,
            "heating COP should be 5.0, got {cop}"
        );
    }

    #[test]
    fn hvac_cooling_cop_uses_abs_of_negative_delivery() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Cooling Electric Power (kW)",
            "HVAC Cooling Delivered (W)",
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");

        // -8000 W cooling (negative = heat removal), 2 kW electric → COP = 8000/2000 = 4.0
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![2.0]),
            ("HVAC Cooling Electric Power (kW)", vec![2.0]),
            ("HVAC Cooling Delivered (W)", vec![-8_000.0]),
        ]));
        let metrics = calc.finish();

        let cop = metrics
            .metrics
            .efficiency
            .hvac_cooling_cop
            .expect("COP should be present");
        assert!(
            (cop - 4.0).abs() < 1e-9,
            "cooling COP should be 4.0, got {cop}"
        );
    }

    #[test]
    fn cop_none_when_no_electric_draw() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "HVAC Heating Delivered (W)"]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![0.0]),
            ("HVAC Heating Delivered (W)", vec![5000.0]),
        ]));
        let metrics = calc.finish();

        assert!(metrics.metrics.efficiency.hvac_heating_cop.is_none());
    }

    #[test]
    fn battery_round_trip_efficiency_computed() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "Battery Electric Power (kW)"]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");

        // 10 kWh in, 9 kWh out → 90% round trip
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![10.0, -9.0]),
            ("Battery Electric Power (kW)", vec![10.0, -9.0]),
        ]));
        let metrics = calc.finish();

        let rte = metrics
            .metrics
            .efficiency
            .battery_round_trip_efficiency
            .expect("RTE should be present");
        assert!((rte - 0.9).abs() < 1e-9, "RTE should be 0.9, got {rte}");
    }

    #[test]
    fn ventilation_accumulates_forced_and_natural() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Forced Ventilation Heat Gain - Indoor (W)",
            "Natural Ventilation Heat Gain - Indoor (W)",
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");

        // 500 W forced + 300 W natural = 800 W → 800 Wh → 0.8 kWh for 1 hour
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0]),
            ("Forced Ventilation Heat Gain - Indoor (W)", vec![500.0]),
            ("Natural Ventilation Heat Gain - Indoor (W)", vec![300.0]),
        ]));
        let metrics = calc.finish();

        let loads = metrics
            .metrics
            .envelope_loads_kwh
            .expect("envelope loads present");
        assert!(
            (loads.ventilation_kwh - 0.8).abs() < 1e-9,
            "ventilation should be 0.8 kWh, got {}",
            loads.ventilation_kwh
        );
    }

    #[test]
    fn envelope_loads_include_all_components() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Window Transmitted Solar Gain (W)",
            "Opaque Surface Heat Gain - Indoor (W)",
            "Interior LWR Exchange - Indoor (W)",
            "Infiltration Heat Gain - Indoor (W)",
            "Forced Ventilation Heat Gain - Indoor (W)",
            "Internal Heat Gain - Indoor (W)",
            "Duct Loss Heat Gain - Indoor (W)",
            "HVAC Heating Delivered (W)",
            "HVAC Cooling Delivered (W)",
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");

        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0]),
            ("Window Transmitted Solar Gain (W)", vec![1000.0]),
            ("Opaque Surface Heat Gain - Indoor (W)", vec![200.0]),
            ("Interior LWR Exchange - Indoor (W)", vec![50.0]),
            ("Infiltration Heat Gain - Indoor (W)", vec![-100.0]),
            ("Forced Ventilation Heat Gain - Indoor (W)", vec![-50.0]),
            ("Internal Heat Gain - Indoor (W)", vec![300.0]),
            ("Duct Loss Heat Gain - Indoor (W)", vec![75.0]),
            ("HVAC Heating Delivered (W)", vec![5000.0]),
            ("HVAC Cooling Delivered (W)", vec![-3000.0]),
        ]));
        let metrics = calc.finish();
        let loads = metrics
            .metrics
            .envelope_loads_kwh
            .expect("envelope loads present");

        assert!((loads.window_solar_kwh - 1.0).abs() < 1e-9);
        assert!((loads.opaque_solar_lwr_kwh - 0.2).abs() < 1e-9);
        assert!((loads.interior_lwr_kwh - 0.05).abs() < 1e-9);
        assert!((loads.infiltration_kwh - (-0.1)).abs() < 1e-9);
        assert!((loads.ventilation_kwh - (-0.05)).abs() < 1e-9);
        assert!((loads.internal_gains_kwh - 0.3).abs() < 1e-9);
        assert!((loads.duct_loss_kwh - 0.075).abs() < 1e-9);
        assert!((loads.hvac_heating_kwh - 5.0).abs() < 1e-9);
        assert!((loads.hvac_cooling_kwh - (-3.0)).abs() < 1e-9);
    }

    #[test]
    fn envelope_loads_none_without_any_envelope_columns() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        calc.accumulate(&build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![1.0])]));
        let metrics = calc.finish();
        assert!(metrics.metrics.envelope_loads_kwh.is_none());
    }

    // ── End‑use aggregate column tests ─────────────────────────────────

    /// Aggregate columns are discovered with EndUse keys, per-equipment
    /// columns are skipped. Verifies that `discover_end_use_columns` correctly
    /// maps aggregate column display names back to EndUse category strings.
    #[test]
    fn discover_end_use_columns_maps_aggregate_not_per_equipment() {
        // Schema with both per-equipment and aggregate columns.
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "ASHP Heater Electric Power (kW)",
            "Gas Furnace Electric Power (kW)",
            "HVAC Heating Electric Power (kW)",
            "Battery Electric Power (kW)",
        ]);
        let calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        let metrics = calc.finish();

        // energy_by_end_use should have "hvac_heating" and "battery" keys
        // (from aggregate columns), NOT "ASHP Heater" or "Gas Furnace"
        // (from per-equipment columns).
        assert!(
            metrics
                .annual_energy_kwh
                .per_end_use
                .contains_key("hvac_heating"),
            "per_end_use must contain key 'hvac_heating' from aggregate column"
        );
        assert!(
            metrics
                .annual_energy_kwh
                .per_end_use
                .contains_key("battery"),
            "per_end_use must contain key 'battery' from aggregate column"
        );
        assert!(
            !metrics
                .annual_energy_kwh
                .per_end_use
                .contains_key("ASHP Heater"),
            "per_end_use must NOT contain per-equipment key 'ASHP Heater'"
        );
        assert!(
            !metrics
                .annual_energy_kwh
                .per_end_use
                .contains_key("Gas Furnace"),
            "per_end_use must NOT contain per-equipment key 'Gas Furnace'"
        );
    }

    /// Multiple HVAC_HEATING equipment power contributions are accumulated
    /// under the same `"hvac_heating"` end-use key.
    #[test]
    fn energy_by_end_use_aggregates_multiple_hvac_heating_equipment() {
        let schema =
            schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "HVAC Heating Electric Power (kW)"]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        // Simulate two hours at constant 3 kW
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![3.0, 3.0]),
            ("HVAC Heating Electric Power (kW)", vec![3.0, 3.0]),
        ]));
        let metrics = calc.finish();
        assert!(
            metrics.annual_energy_kwh.per_end_use["hvac_heating"] > 0.0,
            "hvac_heating energy must be non-zero"
        );
        assert!(
            (metrics.annual_energy_kwh.per_end_use["hvac_heating"] - 6.0).abs() < 1e-9,
            "hvac_heating energy must equal 3 kW × 2 h = 6 kWh"
        );
    }

    /// HVAC heating electric energy is populated when the aggregate column
    /// has positive values. Regression test for the hvac_heating_kw_idx
    /// resolution and hvac_heating_electric_wh accumulation.
    #[test]
    fn hvac_heating_electric_wh_populated_from_aggregate_column() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating Electric Power (kW)",
            "HVAC Heating Delivered (W)",
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![3.0, 3.0]),
            ("HVAC Heating Electric Power (kW)", vec![2.0, 2.5]),
            ("HVAC Heating Delivered (W)", vec![6000.0, 7000.0]),
        ]));
        let metrics = calc.finish();
        // hvac_heating_cop requires hvac_heating_electric_wh > 0
        assert!(
            metrics.efficiency.hvac_heating_cop.is_some(),
            "hvac_heating_cop must be Some when HVAC Heating electric power > 0; \
             hvac_heating_kw_idx resolution failed"
        );
        assert!(
            metrics.efficiency.hvac_heating_cop.unwrap() > 0.0,
            "hvac_heating_cop must be > 0"
        );
    }

    // ── Equipment-count derivation from schema ─────────────────────────

    /// `schema_has_equipment_for` detects HVAC equipment columns in the
    /// schema and correctly returns false when none are present.
    #[test]
    fn schema_has_equipment_for_detects_hvac_columns() {
        let hvac_schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "ASHP Heater Electric Power (kW)",
            "HVAC Heating Electric Power (kW)",
        ]);
        assert!(
            schema_has_equipment_for(&hvac_schema, &hares_types::EndUse::HVAC_HEATING),
            "must detect ASHP Heater as HVAC_HEATING equipment"
        );

        // Battery is not HVAC.
        let battery_schema =
            schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "Battery Electric Power (kW)"]);
        assert!(
            !schema_has_equipment_for(&battery_schema, &hares_types::EndUse::HVAC_HEATING),
            "must NOT detect Battery as HVAC_HEATING equipment"
        );
        assert!(
            !schema_has_equipment_for(&battery_schema, &hares_types::EndUse::HVAC_COOLING),
            "must NOT detect Battery as HVAC_COOLING equipment"
        );

        // Empty schema has no equipment.
        let empty = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        assert!(
            !schema_has_equipment_for(&empty, &hares_types::EndUse::HVAC_HEATING),
            "empty schema must not report HVAC equipment"
        );
    }

    /// `schema_has_equipment_for` strips instance qualifiers (" #N") from
    /// multi-instance equipment column names.
    #[test]
    fn schema_has_equipment_for_strips_instance_qualifiers() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "ASHP Heater #1 Electric Power (kW)",
            "ASHP Heater #2 Electric Power (kW)",
            "HVAC Heating Electric Power (kW)",
        ]);
        assert!(
            schema_has_equipment_for(&schema, &hares_types::EndUse::HVAC_HEATING),
            "must detect instance-qualified ASHP Heater columns as HVAC_HEATING"
        );
    }

    /// `compute_equipment_counts_from_schema` counts per-equipment columns
    /// grouped by EndUse category, skipping aggregate columns.
    #[test]
    #[cfg(feature = "observe")]
    fn compute_equipment_counts_counts_equipment_excluding_aggregates() {
        // Schema with 2 HVAC_HEATING equipment, 1 BATTERY, and aggregate columns.
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "ASHP Heater Electric Power (kW)",
            "Gas Furnace Electric Power (kW)",
            "HVAC Heating Electric Power (kW)",
            "Battery Electric Power (kW)",
        ]);
        let counts = compute_equipment_counts_from_schema(&schema);
        assert_eq!(
            counts.get("hvac_heating"),
            Some(&2),
            "must count ASHP Heater + Gas Furnace = 2 HVAC_HEATING equipment"
        );
        assert_eq!(
            counts.get("battery"),
            Some(&1),
            "must count 1 Battery equipment"
        );
        // Aggregate columns (e.g. "HVAC Heating") must not be counted.
        assert!(
            !counts.contains_key("other"),
            "aggregate columns must not be counted as OTHER: got keys {:?}",
            counts.keys().collect::<Vec<_>>()
        );
    }

    /// `MetricsCalculator::new` populates `end_use_equipment_counts` from
    /// the schema so observer diagnostics report non-zero equipment counts.
    #[test]
    #[cfg(feature = "observe")]
    fn metrics_calculator_new_populates_equipment_counts() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "ASHP Heater Electric Power (kW)",
            "HVAC Heating Electric Power (kW)",
        ]);
        let calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
        assert!(
            !calc.end_use_equipment_counts.is_empty(),
            "end_use_equipment_counts must be populated from schema"
        );
        assert_eq!(
            calc.end_use_equipment_counts.get("hvac_heating"),
            Some(&1),
            "must count 1 HVAC_HEATING equipment"
        );
    }
}
