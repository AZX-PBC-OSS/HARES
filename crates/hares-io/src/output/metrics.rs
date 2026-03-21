//! Derived metrics and post-processing.

use std::collections::BTreeMap;

use arrow::{
    array::{Array, Float64Array},
    datatypes::{DataType, Schema},
    record_batch::RecordBatch,
};
use thiserror::Error;

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

/// Final simulation metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulationMetrics {
    pub annual_energy_kwh: AnnualEnergyKwh,
    pub peak_power_kw: PeakPowerKw,
    pub comfort_hours: Option<f64>,
    pub unmet_load_hours: Option<f64>,
    pub renewable_energy_fraction: Option<f64>,
    pub grid_interaction_metrics: GridInteractionMetrics,
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
        })
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
        // OCHRE-style: "{End Use} Electric Power (kW)"
        if name.ends_with(ELECTRIC_POWER_SUFFIX) {
            ensure_float64(schema, idx)?;
            let end_use = name
                .strip_suffix(ELECTRIC_POWER_SUFFIX)
                .unwrap_or(name)
                .trim()
                .to_owned();
            columns.push((end_use, idx));
        }
    }
    Ok(columns)
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
        optional_float64_column(schema, &["HVAC Heating Capacity (W)"])?,
    ) {
        pairs.push((output_idx, capacity_idx));
    }
    // Try cooling pair.
    if let (Some(output_idx), Some(capacity_idx)) = (
        optional_float64_column(schema, &["HVAC Cooling Delivered (W)"])?,
        optional_float64_column(schema, &["HVAC Cooling Capacity (W)"])?,
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
            start_time: FixedOffset::east_opt(0).unwrap().with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap(),
            duration: Duration::hours(1),
            time_res: Duration::hours(1),
            output_verbosity: 0,
            output_path: None,
            output_format: crate::OutputFormat::Csv,
            output_chunk_size: 16,
            master_seed: 0,
            setpoint_deadband_c: deadband,
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

        assert_eq!(metrics.peak_power_kw.per_end_use["EV"], 6.2);
    }

    #[test]
    fn comfort_hours_equals_duration_when_all_inside_deadband() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Temperature - Indoor (C)",
            "HVAC Heating Setpoint (C)",
            "HVAC Cooling Setpoint (C)",
            "HVAC Heating Delivered (W)",
            "HVAC Heating Capacity (W)",
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(Some(1.0))).expect("new");

        let batch = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0, 1.0]),
            ("Temperature - Indoor (C)", vec![21.0, 21.4, 22.0]),
            ("HVAC Heating Setpoint (C)", vec![21.0, 21.0, 21.0]),
            ("HVAC Cooling Setpoint (C)", vec![22.0, 22.0, 22.0]),
            ("HVAC Heating Delivered (W)", vec![0.0, 0.0, 0.0]),
            ("HVAC Heating Capacity (W)", vec![4.0, 4.0, 4.0]),
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
            "HVAC Heating Capacity (W)",
        ]);
        let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");

        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0, 1.0]),
            ("Temperature - Indoor (C)", vec![24.0, 22.0, 24.0]),
            ("HVAC Heating Setpoint (C)", vec![21.0, 21.0, 21.0]),
            ("HVAC Cooling Setpoint (C)", vec![22.0, 22.0, 22.0]),
            ("Setpoint Deadband (C)", vec![1.0, 1.0, 1.0]),
            ("HVAC Heating Delivered (W)", vec![3.0, 2.0, 3.0]),
            ("HVAC Heating Capacity (W)", vec![3.0, 3.0, 3.0]),
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
            name: "ASHP Heater".to_string(),
            fuel_type: hares_types::FuelType::Electric,
            parameters: serde_json::Map::new(),
            zip_params: None,
        }];
        let schema = crate::output::build_schema(&specs, 1);
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
            "HVAC Heating Capacity (W)",
        ]);
        let mut calc =
            MetricsCalculator::new(&schema, 3600, &test_config(Some(1.0))).expect("new");

        let rows = 24;
        calc.accumulate(&build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![2.0; rows]),
            (TOTAL_GAS_POWER_THERMS, vec![0.5; rows]),
            ("Temperature - Indoor (C)", vec![21.5; rows]),
            ("HVAC Heating Setpoint (C)", vec![21.0; rows]),
            ("HVAC Cooling Setpoint (C)", vec![23.0; rows]),
            ("HVAC Heating Delivered (W)", vec![1.0; rows]),
            ("HVAC Heating Capacity (W)", vec![5.0; rows]),
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
            "HVAC Heating Capacity (W)",
        ]);
        let mut calc =
            MetricsCalculator::new(&schema, 3600, &test_config(Some(1.0))).expect("new");

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
            ("HVAC Heating Capacity (W)", vec![capacity]),
        ]));
        let metrics = calc.finish();

        assert_eq!(
            metrics.unmet_load_hours,
            Some(1.0),
            "should detect capacity saturation at high wattage"
        );
    }
}
