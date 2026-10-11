//! Derived run metrics, computed only when the run requests them.
//!
//! One path: a fixed [`MetricsInputPlan`] (the columns the metrics read,
//! resolved once against the run's producer surface, which the catalogue
//! derives) is built at freeze, and the [`MetricsCalculator`] accumulator
//! is fed per step from the row plan's producer slots. The state is exact
//! at every step, and a run that does not request metrics builds no plan
//! and no accumulator and pays nothing.
//!
//! The one accumulator has two feeds: the run feeds it per step from the
//! row plan's producer slots, and [`metrics_from_table`] feeds it from
//! any Timestep results table, its columns resolved through the
//! catalogue. The two feeds run the same aggregations and produce the
//! same values: a table the run itself recorded scores the run's metrics
//! on bits.
//!
//! The table feed's exact-delta gate checks consecutive `Time` pairs
//! only, so a table truncated at either end passes it: there is no
//! start anchor. The caller owns the table's extent; the coverage year
//! reads the first `Time` value.

use std::collections::BTreeMap;

use arrow::array::{Array, Float64Array, RecordBatch, TimestampMillisecondArray};
use arrow::datatypes::DataType;
use chrono::{DateTime, Datelike, Duration};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use hares_physics::constants::HOURS_PER_YEAR;
use hares_physics::units::energy_therms_to_kwh;
use hares_types::telemetry_keys as tk;

use crate::SimulationConfig;
use crate::output::catalogue::{Catalogue, time_field, variable_field};
#[cfg(feature = "observe")]
use crate::output::columns::ELECTRIC_POWER_SUFFIX;

const TOTAL_ELECTRIC_POWER_KW: &str = "Total Electric Power (kW)";
const TOTAL_GAS_POWER_THERMS: &str = "Total Gas Power (therms/hour)";
const GRID_ELECTRIC_POWER_KW: &str = "Grid Electric Power (kW)";
const SETPOINT_DEADBAND_C: &str = "Setpoint Deadband (C)";
const DEADBAND_C: &str = "Deadband (C)";

/// The reduced tables' step-count column: a period row's step count. The
/// catalogue declares no such variable, so the Timestep table check names
/// this column when it finds one.
const STEP_COUNT_COLUMN: &str = "Step Count";

const EPSILON: f64 = 1e-9;

/// Errors returned by [`MetricsInputPlan::build`],
/// [`MetricsCalculator::new`] and [`metrics_from_table`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MetricsError {
    #[error("metrics snapshot mismatch: {0}")]
    Snapshot(String),
    #[error("time_res_secs must be greater than zero")]
    InvalidTimeResolution,
    #[error("required metrics column missing: {0}")]
    MissingRequiredColumn(String),
    #[error("metrics accumulator `{0}` is non-finite")]
    NonFiniteAccumulator(String),
    #[error(
        "table column `{column}` is {found}, contradicting the catalogue's {expected} declaration"
    )]
    ColumnTypeMismatch {
        column: String,
        expected: String,
        found: String,
    },
    #[error("not a Timestep table: {0}")]
    NonTimestepTable(String),
    #[error("the table is empty: the metrics are computed from a Timestep results table's rows")]
    EmptyTable,
    #[error(
        "table batches do not share one schema: batch {batch} differs from the first at column `{column}`"
    )]
    InconsistentBatches { batch: usize, column: String },
}

/// Total accumulated electric energy over the simulation period.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TotalEnergyKwh {
    /// Net electric energy at the meter — gross consumption minus gross PV
    /// generation (kWh). Positive = import, negative = export. The identity
    /// `net == gross_consumption − gross_pv` holds whenever PV is the only
    /// export source; battery discharge to the grid breaks it by design
    /// (the discharged energy is neither load nor PV).
    pub net_energy_kwh: f64,
    /// Gross electric consumption — the load behind the meter: the net
    /// total plus self-consumed PV generation, which never appears in the
    /// net column (kWh). Non-negative per row; a row where a non-PV source
    /// exports harder than the load contributes zero.
    pub gross_consumption_kwh: f64,
    /// Gross PV generation — abs of negative PV power values (kWh).
    pub gross_pv_generation_kwh: f64,
    /// Energy by end-use key.
    pub per_end_use: BTreeMap<String, f64>,
    /// Actual simulation duration in hours.
    pub duration_hours: f64,
}

/// Rolling-window peak demand metrics at standard demand intervals.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RollingPeakKw {
    /// Peak 15-minute average demand (kW).
    pub peak_15min_kw: f64,
    /// Peak 30-minute average demand (kW).
    pub peak_30min_kw: f64,
    /// Peak 60-minute average demand (kW).
    pub peak_60min_kw: f64,
}

/// Peak power metrics.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PeakPowerKw {
    /// Peak instantaneous power by end-use key.
    pub per_end_use: BTreeMap<String, f64>,
    /// Rolling-window peak demand metrics for grid/total power.
    pub rolling: RollingPeakKw,
}

/// Grid interaction demand metrics.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GridInteractionMetrics {
    /// Maximum grid import (positive draw) over the run.
    pub peak_import_kw: f64,
    /// Maximum grid export (positive feed-in) over the run.
    pub peak_export_kw: f64,
}

/// Gas energy metrics, available when a gas power column is present.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GasEnergyMetrics {
    /// Total annual gas energy in therms.
    pub total_therms: f64,
    /// Gas energy converted to kWh via the NIST-exact conversion:
    /// 1 therm = 100,000 BTU(IT) ≈ 29.3071 kWh
    /// (see `hares_physics::units::energy_therms_to_kwh`).
    pub total_kwh_equivalent: f64,
}

/// Envelope component loads (kWh) over the simulation period.
///
/// These represent the net thermal loads imposed on the conditioned zone by
/// each envelope component: positive = heat gain to zone, negative = heat
/// loss. Together with the direct-injection fields (infiltration,
/// ventilation, HVAC, internal gains, duct loss) they approximate the zone
/// air heat balance to within the semi-implicit/TARP coupling split, which
/// is reported separately as the "Zone Air Heat Balance Residual (W)"
/// column rather than in any of these fields. The exception is
/// [`Self::interior_lwr_kwh`], a gross exchange metric that is documented inline
/// and must not be summed into a net balance.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct EnvelopeComponentLoadsKwh {
    /// Window transmitted + absorbed-inward solar gain.
    pub window_solar_kwh: f64,
    /// Window conduction into the zone (interior-surface convection —
    /// OCHRE's "Window" component-load category, which covers conduction
    /// plus radiation from the window surface; the solar component is
    /// `window_solar_kwh`).
    pub window_conduction_kwh: f64,
    /// Net conduction from the opaque envelope (wall + floor + roof interior
    /// surfaces) into the conditioned zone. This is the time-lagged fraction
    /// of exterior solar, longwave, and ΔT conduction that actually reaches
    /// zone air — not the gross exterior absorption, which is a boundary
    /// condition on the exterior RC nodes and is reported separately in
    /// observer diagnostics (`EnvelopeComponentGains::opaque_solar_lwr_w`).
    pub opaque_conduction_kwh: f64,
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
    /// Net convection from internal-mass surfaces to zone air — OCHRE's
    /// "Internal Mass" component-load category. A zone air balance term
    /// (heat redistributed between zone air and thermal mass), not an
    /// exterior envelope load.
    pub internal_mass_kwh: f64,
}

/// Per-equipment efficiency metrics.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
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

/// Whether the simulation period covers a full calendar year.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SimulationCoverage {
    /// Full non-leap calendar year (8760 h).
    FullYear,
    /// Full leap calendar year (8784 h); energy totals are normalized to 8760 h.
    LeapYear,
    /// Fewer than 8760 hours; totals represent a partial year.
    PartialYear,
    /// More than 8784 hours; totals aggregate multiple years.
    MultiYear,
}

/// Whether the simulation metrics are fully reliable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Reliability {
    /// All power timesteps were finite — no accumulators were poisoned.
    Reliable,
    /// One or more power timesteps contained non-finite values that were
    /// skipped; downstream consumers should check `nan_step_count` before
    /// trusting aggregate totals.
    Degraded,
}

impl Reliability {
    /// Returns `true` if the reliability status is `Reliable`.
    #[must_use]
    pub fn is_reliable(&self) -> bool {
        matches!(self, Reliability::Reliable)
    }
}

/// Final simulation metrics.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimulationMetrics {
    pub total_energy_kwh: TotalEnergyKwh,
    pub peak_power_kw: PeakPowerKw,
    pub comfort_hours: Option<f64>,
    pub unmet_load_hours: Option<f64>,
    /// Fraction of the gross load (consumption behind the meter) covered by
    /// PV generation. Can exceed 1.0 when generation exceeds load (net-positive
    /// home); 0.0 when no consumption was recorded. `None` without a PV column.
    pub renewable_energy_fraction: Option<f64>,
    pub grid_interaction_metrics: GridInteractionMetrics,
    /// Envelope component loads. `None` if component gain columns not present.
    pub envelope_loads_kwh: Option<EnvelopeComponentLoadsKwh>,
    /// Per-equipment efficiency metrics. Always present (fields are Option).
    pub efficiency: EfficiencyMetrics,
    /// Fraction of rows that lacked valid setpoint data but still
    /// accumulated envelope/HVAC/battery energy. `None` if setpoints
    /// are not configured.
    pub rows_with_partial_setpoint_data_fraction: Option<f64>,
    /// Actual simulation duration in hours as computed from accumulated rows.
    pub simulation_duration_hours: f64,
    /// Whether the simulation period is a full calendar year.
    pub coverage: SimulationCoverage,
    /// Number of power-value timesteps skipped due to non-finite (NaN/Inf) values.
    pub nan_step_count: u64,
    /// Reliability status: `Degraded` if any power timesteps were skipped.
    pub metrics_reliability: Reliability,
}

/// Extended metrics returned by [`MetricsCalculator::finish`], including gas tracking.
///
/// Derefs to [`SimulationMetrics`] for backward-compatible field access.
#[derive(Debug, Clone, PartialEq, Serialize)]
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

    /// Combined total energy: electric kWh + gas kWh equivalent.
    #[must_use]
    pub fn combined_total_energy_kwh(&self) -> f64 {
        self.metrics.total_energy_kwh.net_energy_kwh
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
#[derive(Debug, Clone)]
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

    /// The buffer's full mutable state: the ring's contents and position,
    /// the counts and the running peaks.
    fn snapshot(&self) -> MetricsRollingPeakSnapshot {
        MetricsRollingPeakSnapshot {
            buffer: self.buffer.clone(),
            head: self.head,
            count: self.count,
            peak_15min_kw: self.peak_15min_kw,
            peak_30min_kw: self.peak_30min_kw,
            peak_60min_kw: self.peak_60min_kw,
        }
    }

    /// Restores the buffer's mutable state from its snapshot. The window
    /// sizes are the receiver's own (derived from the time resolution),
    /// and the snapshot's ring must have been built at the same
    /// resolution: the plan comparison ahead of the restore pins the
    /// calculator's identity, and a same-plan rebuild sizes the ring
    /// identically. A snapshot whose ring was built at another resolution
    /// is refused in both debug and release: restoring it would leave the
    /// resumed run's peaks zeroed.
    ///
    /// # Errors
    ///
    /// [`MetricsError::Snapshot`] naming both lengths when the snapshot's
    /// ring does not fit this buffer's resolution.
    fn restore(&mut self, snapshot: MetricsRollingPeakSnapshot) -> Result<(), MetricsError> {
        if snapshot.buffer.len() != self.buffer.len() {
            return Err(MetricsError::Snapshot(format!(
                "the checkpoint's rolling peak ring holds {} readings, \
                 this run's time resolution needs {}",
                snapshot.buffer.len(),
                self.buffer.len()
            )));
        }
        self.buffer.copy_from_slice(&snapshot.buffer);
        self.head = snapshot.head % self.buffer.len().max(1);
        self.count = snapshot.count.min(self.buffer.len());
        self.peak_15min_kw = snapshot.peak_15min_kw;
        self.peak_30min_kw = snapshot.peak_30min_kw;
        self.peak_60min_kw = snapshot.peak_60min_kw;
        Ok(())
    }
}

/// The metrics' fixed input plan: every column the metrics read, resolved
/// once against the run's producer surface in producer-slot order. The
/// surface is the union of the selected reports' variables (for a run with
/// no reports, the `Verbosity(0)` preset, the minimum output always
/// contained); each metric whose availability depends on a column being
/// present keeps that dependence, evaluated against the same surface.
#[derive(Debug, Clone)]
pub struct MetricsInputPlan {
    total_electric_power_slot: usize,
    total_gas_power_slot: Option<usize>,
    grid_power_slot: usize,
    pv_power_slot: Option<usize>,
    end_use_columns: Vec<(String, usize)>,
    setpoints: Option<SetpointInputs>,
    hvac_capacity_pairs: Vec<(usize, usize)>,

    // Envelope component gain slots (optional -- verbosity >= 6)
    window_solar_w_slot: Option<usize>,
    infiltration_w_slot: Option<usize>,
    interior_lwr_w_slot: Option<usize>,
    internal_gains_w_slot: Option<usize>,
    opaque_conduction_w_slot: Option<usize>,
    window_conduction_w_slot: Option<usize>,
    internal_mass_w_slot: Option<usize>,
    forced_ventilation_w_slot: Option<usize>,
    natural_ventilation_w_slot: Option<usize>,
    duct_loss_w_slot: Option<usize>,
    // HVAC thermal delivered slots (verbosity >= 4)
    hvac_heating_delivered_w_slot: Option<usize>,
    hvac_cooling_delivered_w_slot: Option<usize>,
    // HVAC electric power and battery slots (discovered from end-use naming)
    hvac_heating_kw_slot: Option<usize>,
    hvac_cooling_kw_slot: Option<usize>,
    battery_kw_slot: Option<usize>,

    #[cfg(feature = "observe")]
    end_use_equipment_counts: BTreeMap<String, usize>,
}

impl MetricsInputPlan {
    /// Builds the plan from the producer surface's variable names, in
    /// producer-slot order, and the simulation config.
    ///
    /// # Errors
    ///
    /// [`MetricsError::MissingRequiredColumn`] when the surface carries no
    /// total electric power column (the metrics' one required input), and
    /// when an HVAC aggregate column is missing while per-equipment columns
    /// for that end use exist (schema drift: the aggregate metric would
    /// silently read zero).
    pub fn build(available: &[String], config: &SimulationConfig) -> Result<Self, MetricsError> {
        Self::build_from_names(available, config.setpoint_deadband_c)
    }

    /// Builds the plan from column names, with the thermostat deadband
    /// given directly: the table feed's constructor, carrying the config
    /// field's value the caller read (`None` leaves the deadband column
    /// the one source, as in-run).
    ///
    /// # Errors
    ///
    /// The same errors [`Self::build`] documents.
    pub fn build_from_names(
        available: &[String],
        setpoint_deadband_c: Option<f64>,
    ) -> Result<Self, MetricsError> {
        let total_electric_power_slot = required_slot(available, TOTAL_ELECTRIC_POWER_KW)?;
        let total_gas_power_slot = optional_slot(available, &[TOTAL_GAS_POWER_THERMS])?;
        let grid_power_slot = optional_slot(available, &[GRID_ELECTRIC_POWER_KW])?
            .unwrap_or(total_electric_power_slot);
        let pv_power_slot = optional_slot(available, &["PV End Use Electric Power (kW)"])?;

        let end_use_columns =
            discover_end_use_slots(available, total_electric_power_slot, grid_power_slot)?;
        let setpoints = discover_setpoint_inputs(available, setpoint_deadband_c)?;
        let hvac_capacity_pairs = discover_hvac_capacity_pairs(available)?;

        // Optional envelope component gain columns (verbosity >= 6).
        let window_solar_w_slot = optional_slot(available, &["Window Transmitted Solar Gain (W)"])?;
        let infiltration_w_slot =
            optional_slot(available, &["Infiltration Heat Gain - Indoor (W)"])?;
        let interior_lwr_w_slot =
            optional_slot(available, &["Interior LWR Exchange - Indoor (W)"])?;
        let internal_gains_w_slot = optional_slot(available, &["Internal Heat Gain - Indoor (W)"])?;
        let opaque_conduction_w_slot =
            optional_slot(available, &["Opaque Surface Heat Gain - Indoor (W)"])?;
        let window_conduction_w_slot =
            optional_slot(available, &["Window Heat Gain - Indoor (W)"])?;
        let internal_mass_w_slot =
            optional_slot(available, &["Internal Mass Heat Gain - Indoor (W)"])?;
        let forced_ventilation_w_slot =
            optional_slot(available, &["Forced Ventilation Heat Gain - Indoor (W)"])?;
        let natural_ventilation_w_slot =
            optional_slot(available, &["Natural Ventilation Heat Gain - Indoor (W)"])?;
        let duct_loss_w_slot = optional_slot(available, &["Duct Loss Heat Gain - Indoor (W)"])?;

        // HVAC thermal delivered columns (verbosity >= 4).
        let hvac_heating_delivered_w_slot =
            optional_slot(available, &["HVAC Heating Delivered (W)"])?;
        let hvac_cooling_delivered_w_slot =
            optional_slot(available, &["HVAC Cooling Delivered (W)"])?;

        // HVAC electric power and battery columns from end-use naming.
        let hvac_heating_kw_slot =
            optional_slot(available, &["HVAC Heating End Use Electric Power (kW)"])?;
        let hvac_cooling_kw_slot =
            optional_slot(available, &["HVAC Cooling End Use Electric Power (kW)"])?;
        let battery_kw_slot = optional_slot(available, &["Battery End Use Electric Power (kW)"])?;

        // A missing HVAC aggregate column while per-equipment columns for
        // that end use exist is schema drift: the aggregate metric would
        // silently read zero.
        {
            let has_hvac_heating_equipment =
                names_have_equipment_for(available, &hares_types::EndUse::HVAC_HEATING);
            let has_hvac_cooling_equipment =
                names_have_equipment_for(available, &hares_types::EndUse::HVAC_COOLING);

            if hvac_heating_kw_slot.is_none() && has_hvac_heating_equipment {
                return Err(MetricsError::MissingRequiredColumn(
                    "HVAC Heating End Use Electric Power (kW)".to_string(),
                ));
            }
            if hvac_cooling_kw_slot.is_none() && has_hvac_cooling_equipment {
                return Err(MetricsError::MissingRequiredColumn(
                    "HVAC Cooling End Use Electric Power (kW)".to_string(),
                ));
            }
        }

        Ok(Self {
            total_electric_power_slot,
            total_gas_power_slot,
            grid_power_slot,
            pv_power_slot,
            end_use_columns,
            setpoints,
            hvac_capacity_pairs,
            window_solar_w_slot,
            infiltration_w_slot,
            interior_lwr_w_slot,
            internal_gains_w_slot,
            opaque_conduction_w_slot,
            window_conduction_w_slot,
            internal_mass_w_slot,
            forced_ventilation_w_slot,
            natural_ventilation_w_slot,
            duct_loss_w_slot,
            hvac_heating_delivered_w_slot,
            hvac_cooling_delivered_w_slot,
            hvac_heating_kw_slot,
            hvac_cooling_kw_slot,
            battery_kw_slot,
            #[cfg(feature = "observe")]
            end_use_equipment_counts: compute_equipment_counts_from_names(available),
        })
    }

    /// The input plan's resolved slots, as the checkpoint section's plan
    /// half; the counterpart a restore compares the rebuilt plan against.
    #[must_use]
    pub fn snapshot(&self) -> MetricsPlanSnapshot {
        MetricsPlanSnapshot {
            total_electric_power_slot: self.total_electric_power_slot,
            total_gas_power_slot: self.total_gas_power_slot,
            grid_power_slot: self.grid_power_slot,
            pv_power_slot: self.pv_power_slot,
            end_use_columns: self.end_use_columns.clone(),
            setpoints: self
                .setpoints
                .as_ref()
                .map(|setpoints| MetricsSetpointSnapshot {
                    heating_setpoint_slot: setpoints.heating_setpoint_slot,
                    cooling_setpoint_slot: setpoints.cooling_setpoint_slot,
                    zone_temperature_slots: setpoints.zone_temperature_slots.clone(),
                    deadband: match &setpoints.deadband {
                        DeadbandSource::Constant(value) => {
                            MetricsDeadbandSnapshot::Constant(*value)
                        }
                        DeadbandSource::Column(slot) => MetricsDeadbandSnapshot::Column(*slot),
                    },
                }),
            hvac_capacity_pairs: self.hvac_capacity_pairs.clone(),
            window_solar_w_slot: self.window_solar_w_slot,
            infiltration_w_slot: self.infiltration_w_slot,
            interior_lwr_w_slot: self.interior_lwr_w_slot,
            internal_gains_w_slot: self.internal_gains_w_slot,
            opaque_conduction_w_slot: self.opaque_conduction_w_slot,
            window_conduction_w_slot: self.window_conduction_w_slot,
            internal_mass_w_slot: self.internal_mass_w_slot,
            forced_ventilation_w_slot: self.forced_ventilation_w_slot,
            natural_ventilation_w_slot: self.natural_ventilation_w_slot,
            duct_loss_w_slot: self.duct_loss_w_slot,
            hvac_heating_delivered_w_slot: self.hvac_heating_delivered_w_slot,
            hvac_cooling_delivered_w_slot: self.hvac_cooling_delivered_w_slot,
            hvac_heating_kw_slot: self.hvac_heating_kw_slot,
            hvac_cooling_kw_slot: self.hvac_cooling_kw_slot,
            battery_kw_slot: self.battery_kw_slot,
        }
    }
}

/// The metrics input plan's resolved slots, as a checkpoint section: the
/// plan a resumed run rebuilds must resolve exactly this, or the
/// checkpoint's accumulators were fed from a different producer surface
/// and its totals would mix two schemas' columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsPlanSnapshot {
    pub total_electric_power_slot: usize,
    pub total_gas_power_slot: Option<usize>,
    pub grid_power_slot: usize,
    pub pv_power_slot: Option<usize>,
    pub end_use_columns: Vec<(String, usize)>,
    pub setpoints: Option<MetricsSetpointSnapshot>,
    pub hvac_capacity_pairs: Vec<(usize, usize)>,
    pub window_solar_w_slot: Option<usize>,
    pub infiltration_w_slot: Option<usize>,
    pub interior_lwr_w_slot: Option<usize>,
    pub internal_gains_w_slot: Option<usize>,
    pub opaque_conduction_w_slot: Option<usize>,
    pub window_conduction_w_slot: Option<usize>,
    pub internal_mass_w_slot: Option<usize>,
    pub forced_ventilation_w_slot: Option<usize>,
    pub natural_ventilation_w_slot: Option<usize>,
    pub duct_loss_w_slot: Option<usize>,
    pub hvac_heating_delivered_w_slot: Option<usize>,
    pub hvac_cooling_delivered_w_slot: Option<usize>,
    pub hvac_heating_kw_slot: Option<usize>,
    pub hvac_cooling_kw_slot: Option<usize>,
    pub battery_kw_slot: Option<usize>,
}

/// The setpoint inputs' resolved slots, as part of [`MetricsPlanSnapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsSetpointSnapshot {
    pub heating_setpoint_slot: usize,
    pub cooling_setpoint_slot: usize,
    pub zone_temperature_slots: Vec<usize>,
    pub deadband: MetricsDeadbandSnapshot,
}

/// The deadband's source, as part of [`MetricsSetpointSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MetricsDeadbandSnapshot {
    Constant(f64),
    Column(usize),
}

/// The metrics accumulator's mutable state, exact at the checkpoint step:
/// the section a checkpoint restores so a resumed run's metrics finalize
/// to the continuous run's, every float on bits.
///
/// A non-finite peak value is the one state the JSON cannot carry: the
/// calculator's `peak_by_end_use` holds a `NEG_INFINITY` sentinel for
/// every end use that has read no finite power yet, so peaks travel as
/// `Option<f64>` with `None` for a non-finite value (the sentinel; no
/// other non-finite can sit there, and a non-finite accumulator elsewhere
/// fails the run).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsAccumulatorsSnapshot {
    pub total_electric_energy_kwh: f64,
    pub total_gas_energy_therms: f64,
    pub total_consumption_kwh: f64,
    pub total_pv_generation_kwh_abs: f64,
    pub peak_import_kw: f64,
    pub peak_export_kw: f64,
    pub energy_by_end_use: BTreeMap<String, f64>,
    pub peak_by_end_use: BTreeMap<String, Option<f64>>,
    pub comfort_step_count: f64,
    pub unmet_step_count: f64,
    pub rolling_peak: MetricsRollingPeakSnapshot,
    pub envelope_window_solar_wh: f64,
    pub envelope_opaque_conduction_wh: f64,
    pub envelope_window_conduction_wh: f64,
    pub envelope_internal_mass_wh: f64,
    pub envelope_interior_lwr_wh: f64,
    pub envelope_infiltration_wh: f64,
    pub envelope_ventilation_wh: f64,
    pub envelope_hvac_heating_wh: f64,
    pub envelope_hvac_cooling_wh: f64,
    pub envelope_internal_gains_wh: f64,
    pub envelope_duct_loss_wh: f64,
    pub hvac_heating_electric_wh: f64,
    pub hvac_cooling_electric_wh: f64,
    pub battery_energy_in_kwh: f64,
    pub battery_energy_out_kwh: f64,
    pub total_row_count: u64,
    pub setpoint_missing_row_count: u64,
    pub nan_step_count: u64,
}

/// The rolling peak buffer's ring state, as part of
/// [`MetricsAccumulatorsSnapshot`]. The window sizes are derived from the
/// time resolution the fresh calculator is built with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsRollingPeakSnapshot {
    pub buffer: Vec<f64>,
    pub head: usize,
    pub count: usize,
    pub peak_15min_kw: f64,
    pub peak_30min_kw: f64,
    pub peak_60min_kw: f64,
}

/// The run metrics' checkpoint section: the input plan's resolved slots
/// and the accumulator state, present exactly when the run requested
/// metrics. The time resolution travels with it: the input plan is
/// resolution-free and two resolutions can size the rolling peak's ring
/// identically, so the resolution is the one gate a plan-and-ring
/// comparison cannot stand in for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub plan: MetricsPlanSnapshot,
    pub accumulators: MetricsAccumulatorsSnapshot,
    pub time_res_secs: u32,
}

/// Streaming post-processing calculator fed one row per step.
#[derive(Debug, Clone)]
pub struct MetricsCalculator {
    timestep_h: f64,
    time_res_secs: u32,
    plan: MetricsInputPlan,

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
    envelope_opaque_conduction_wh: f64,
    envelope_window_conduction_wh: f64,
    envelope_internal_mass_wh: f64,
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

    // Row counters for partial-setpoint diagnostics
    total_row_count: u64,
    setpoint_missing_row_count: u64,

    // The calendar year the coverage reads: the run's start time in-run,
    // the table's first Time value for the table feed.
    coverage_year: i32,

    // Non-finite power value tracking
    nan_step_count: u64,
}

#[derive(Debug, Clone)]
struct SetpointInputs {
    heating_setpoint_slot: usize,
    cooling_setpoint_slot: usize,
    zone_temperature_slots: Vec<usize>,
    deadband: DeadbandSource,
}

#[derive(Debug, Clone)]
enum DeadbandSource {
    Constant(f64),
    Column(usize),
}

impl MetricsCalculator {
    /// Build the metrics accumulator from a resolved input plan, the run's
    /// config deciding the coverage year (the leap-year normalisation's
    /// calendar year).
    ///
    /// # Errors
    ///
    /// [`MetricsError::InvalidTimeResolution`] for a zero time resolution.
    pub fn new(
        plan: MetricsInputPlan,
        time_res_secs: u32,
        config: &SimulationConfig,
    ) -> Result<Self, MetricsError> {
        Self::for_coverage_year(plan, time_res_secs, config.start_time.year())
    }

    /// Build the accumulator with the coverage year given directly: the
    /// table feed's constructor, the year of the table's first `Time`
    /// value standing in for the run's start time.
    ///
    /// # Errors
    ///
    /// [`MetricsError::InvalidTimeResolution`] for a zero time resolution.
    pub fn for_coverage_year(
        plan: MetricsInputPlan,
        time_res_secs: u32,
        coverage_year: i32,
    ) -> Result<Self, MetricsError> {
        if time_res_secs == 0 {
            return Err(MetricsError::InvalidTimeResolution);
        }

        let timestep_h = f64::from(time_res_secs) / 3600.0;
        let energy_by_end_use = plan
            .end_use_columns
            .iter()
            .map(|(name, _)| (name.clone(), 0.0))
            .collect();
        let peak_by_end_use = plan
            .end_use_columns
            .iter()
            .map(|(name, _)| (name.clone(), f64::NEG_INFINITY))
            .collect();
        let rolling_peak = RollingPeakBuffer::new(timestep_h);

        let has_envelope_columns = plan.window_solar_w_slot.is_some()
            || plan.infiltration_w_slot.is_some()
            || plan.interior_lwr_w_slot.is_some()
            || plan.internal_gains_w_slot.is_some()
            || plan.opaque_conduction_w_slot.is_some()
            || plan.window_conduction_w_slot.is_some()
            || plan.internal_mass_w_slot.is_some()
            || plan.forced_ventilation_w_slot.is_some()
            || plan.natural_ventilation_w_slot.is_some()
            || plan.duct_loss_w_slot.is_some()
            || plan.hvac_heating_delivered_w_slot.is_some()
            || plan.hvac_cooling_delivered_w_slot.is_some();

        Ok(Self {
            timestep_h,
            time_res_secs,
            plan,
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
            envelope_opaque_conduction_wh: 0.0,
            envelope_window_conduction_wh: 0.0,
            envelope_internal_mass_wh: 0.0,
            envelope_interior_lwr_wh: 0.0,
            envelope_infiltration_wh: 0.0,
            envelope_ventilation_wh: 0.0,
            envelope_hvac_heating_wh: 0.0,
            envelope_hvac_cooling_wh: 0.0,
            envelope_internal_gains_wh: 0.0,
            envelope_duct_loss_wh: 0.0,
            has_envelope_columns,
            hvac_heating_electric_wh: 0.0,
            hvac_cooling_electric_wh: 0.0,
            battery_energy_in_kwh: 0.0,
            battery_energy_out_kwh: 0.0,
            total_row_count: 0,
            setpoint_missing_row_count: 0,
            coverage_year,
            nan_step_count: 0,
        })
    }

    /// The rows fed so far: one per step of the run.
    #[must_use]
    pub fn rows_fed(&self) -> u64 {
        self.total_row_count
    }

    /// The accumulator's full mutable state, exact at the current step,
    /// with the input plan's resolved slots: the run metrics' checkpoint
    /// section. The counterpart of [`Self::apply_snapshot`].
    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            plan: self.plan.snapshot(),
            time_res_secs: self.time_res_secs,
            accumulators: MetricsAccumulatorsSnapshot {
                total_electric_energy_kwh: self.total_electric_energy_kwh,
                total_gas_energy_therms: self.total_gas_energy_therms,
                total_consumption_kwh: self.total_consumption_kwh,
                total_pv_generation_kwh_abs: self.total_pv_generation_kwh_abs,
                peak_import_kw: self.peak_import_kw,
                peak_export_kw: self.peak_export_kw,
                energy_by_end_use: self.energy_by_end_use.clone(),
                peak_by_end_use: self
                    .peak_by_end_use
                    .iter()
                    .map(|(name, peak)| (name.clone(), peak_finite(*peak)))
                    .collect(),
                comfort_step_count: self.comfort_step_count,
                unmet_step_count: self.unmet_step_count,
                rolling_peak: self.rolling_peak.snapshot(),
                envelope_window_solar_wh: self.envelope_window_solar_wh,
                envelope_opaque_conduction_wh: self.envelope_opaque_conduction_wh,
                envelope_window_conduction_wh: self.envelope_window_conduction_wh,
                envelope_internal_mass_wh: self.envelope_internal_mass_wh,
                envelope_interior_lwr_wh: self.envelope_interior_lwr_wh,
                envelope_infiltration_wh: self.envelope_infiltration_wh,
                envelope_ventilation_wh: self.envelope_ventilation_wh,
                envelope_hvac_heating_wh: self.envelope_hvac_heating_wh,
                envelope_hvac_cooling_wh: self.envelope_hvac_cooling_wh,
                envelope_internal_gains_wh: self.envelope_internal_gains_wh,
                envelope_duct_loss_wh: self.envelope_duct_loss_wh,
                hvac_heating_electric_wh: self.hvac_heating_electric_wh,
                hvac_cooling_electric_wh: self.hvac_cooling_electric_wh,
                battery_energy_in_kwh: self.battery_energy_in_kwh,
                battery_energy_out_kwh: self.battery_energy_out_kwh,
                total_row_count: self.total_row_count,
                setpoint_missing_row_count: self.setpoint_missing_row_count,
                nan_step_count: self.nan_step_count,
            },
        }
    }

    /// Restores the accumulator's mutable state from its checkpoint
    /// section: the fresh calculator this run built keeps its own plan,
    /// config and time resolution, and the checkpoint's plan must resolve
    /// exactly the same slots — a mismatch means the checkpoint was fed
    /// from a different producer surface, and the refusal names it.
    ///
    /// # Errors
    ///
    /// [`MetricsError::Snapshot`] when the checkpoint's input plan differs
    /// from this run's.
    pub fn apply_snapshot(&mut self, snapshot: &MetricsSnapshot) -> Result<(), MetricsError> {
        if snapshot.plan != self.plan.snapshot() {
            return Err(MetricsError::Snapshot(
                "the checkpoint's metrics input plan does not resolve the same \
                 producer slots this run's does"
                    .to_string(),
            ));
        }
        if snapshot.time_res_secs != self.time_res_secs {
            return Err(MetricsError::Snapshot(format!(
                "the checkpoint's metrics were written at time resolution {} s, \
                 this run's is {} s",
                snapshot.time_res_secs, self.time_res_secs
            )));
        }
        let restored = &snapshot.accumulators;
        // The rolling peak's ring refuses a foreign time resolution before
        // anything changes, so the accumulators below take the
        // checkpoint's state only once the whole section is known to fit.
        self.rolling_peak.restore(restored.rolling_peak.clone())?;
        self.total_electric_energy_kwh = restored.total_electric_energy_kwh;
        self.total_gas_energy_therms = restored.total_gas_energy_therms;
        self.total_consumption_kwh = restored.total_consumption_kwh;
        self.total_pv_generation_kwh_abs = restored.total_pv_generation_kwh_abs;
        self.peak_import_kw = restored.peak_import_kw;
        self.peak_export_kw = restored.peak_export_kw;
        self.energy_by_end_use = restored.energy_by_end_use.clone();
        self.peak_by_end_use = restored
            .peak_by_end_use
            .iter()
            .map(|(name, peak)| (name.clone(), peak_non_finite_or(*peak)))
            .collect();
        self.comfort_step_count = restored.comfort_step_count;
        self.unmet_step_count = restored.unmet_step_count;
        self.envelope_window_solar_wh = restored.envelope_window_solar_wh;
        self.envelope_opaque_conduction_wh = restored.envelope_opaque_conduction_wh;
        self.envelope_window_conduction_wh = restored.envelope_window_conduction_wh;
        self.envelope_internal_mass_wh = restored.envelope_internal_mass_wh;
        self.envelope_interior_lwr_wh = restored.envelope_interior_lwr_wh;
        self.envelope_infiltration_wh = restored.envelope_infiltration_wh;
        self.envelope_ventilation_wh = restored.envelope_ventilation_wh;
        self.envelope_hvac_heating_wh = restored.envelope_hvac_heating_wh;
        self.envelope_hvac_cooling_wh = restored.envelope_hvac_cooling_wh;
        self.envelope_internal_gains_wh = restored.envelope_internal_gains_wh;
        self.envelope_duct_loss_wh = restored.envelope_duct_loss_wh;
        self.hvac_heating_electric_wh = restored.hvac_heating_electric_wh;
        self.hvac_cooling_electric_wh = restored.hvac_cooling_electric_wh;
        self.battery_energy_in_kwh = restored.battery_energy_in_kwh;
        self.battery_energy_out_kwh = restored.battery_energy_out_kwh;
        self.total_row_count = restored.total_row_count;
        self.setpoint_missing_row_count = restored.setpoint_missing_row_count;
        self.nan_step_count = restored.nan_step_count;
        Ok(())
    }

    /// The input plan's resolved slots, without the accumulator state: the
    /// half a restore validates against the checkpoint's section before
    /// any state changes.
    #[must_use]
    pub fn plan_snapshot(&self) -> MetricsPlanSnapshot {
        self.plan.snapshot()
    }

    /// The time resolution the calculator was built at, in seconds: what
    /// a metrics snapshot carries, and what a restore refuses a mismatch
    /// with (the input plan is resolution-free, and two distinct
    /// resolutions can size the rolling peak's ring identically, so the
    /// resolution itself is what the gate compares).
    #[must_use]
    pub fn time_res_secs(&self) -> u32 {
        self.time_res_secs
    }

    /// Read the total electric power slot's value, returning `Some` for
    /// finite values and `None` for non-finite (NaN/Inf) values,
    /// incrementing [`nan_step_count`](Self::nan_step_count) on each skip.
    fn guarded_power_value(&mut self, row: &[f64]) -> Option<f64> {
        let v = row[self.plan.total_electric_power_slot];
        if v.is_finite() {
            Some(v)
        } else {
            self.nan_step_count += 1;
            None
        }
    }

    /// Whether any HVAC capacity pair reads at capacity in this row: the
    /// delivered output within the relative tolerance of the capacity.
    /// Column units are watts (e.g. "HVAC Heating Delivered (W)").
    fn hvac_at_capacity(&self, row: &[f64]) -> bool {
        self.plan
            .hvac_capacity_pairs
            .iter()
            .any(|(output_slot, capacity_slot)| {
                match (
                    value_at_slot(row, *output_slot),
                    value_at_slot(row, *capacity_slot),
                ) {
                    (Some(output_w), Some(capacity_w)) if capacity_w.abs() > EPSILON => {
                        let diff = (output_w.abs() - capacity_w.abs()).abs();
                        diff / capacity_w.abs().max(1.0) < HVAC_CAPACITY_REL_TOL
                    }
                    _ => false,
                }
            })
    }

    /// Update metrics from one step's producer row.
    ///
    /// # Errors
    /// A non-finite accumulator after the row is a typed error in every
    /// build profile: a NaN power value contaminated the totals.
    pub fn accumulate_row(&mut self, row: &[f64]) -> Result<(), MetricsError> {
        self.total_row_count += 1;

        // Gross PV generation this step [kW] (PV column is negative when
        // generating). Read once per row: it feeds both the gross
        // generation accumulator and the load-behind-the-meter figure.
        let pv_gen_kw = self
            .plan
            .pv_power_slot
            .and_then(|slot| value_at_slot(row, slot))
            .filter(|pv| *pv < 0.0)
            .map_or(0.0, f64::abs);

        if let Some(total_kw) = self.guarded_power_value(row) {
            self.total_electric_energy_kwh += total_kw * self.timestep_h;
            // Gross consumption is the load behind the meter: the net
            // total plus self-consumed PV generation. Without this, an
            // always-exporting PV home reports 0 consumption even while
            // its loads run on self-consumed solar.
            let load_kw = total_kw + pv_gen_kw;
            if load_kw > 0.0 {
                self.total_consumption_kwh += load_kw * self.timestep_h;
            }
            // Gross PV generation accumulates on the same row
            // eligibility as net and gross consumption (finite meter
            // total): a non-finite total row is a sensor dropout the
            // calculator explicitly skips, and counting PV on it while
            // the other two skip breaks the documented identity
            // net == gross_consumption − gross_pv and skews
            // renewable_energy_fraction.
            if pv_gen_kw > 0.0 {
                self.total_pv_generation_kwh_abs += pv_gen_kw * self.timestep_h;
            }
        }

        if let Some(gas_therms_per_h) = self
            .plan
            .total_gas_power_slot
            .and_then(|slot| value_at_slot(row, slot))
        {
            self.total_gas_energy_therms += gas_therms_per_h * self.timestep_h;
        }

        if let Some(grid_kw) = value_at_slot(row, self.plan.grid_power_slot) {
            if grid_kw > self.peak_import_kw {
                self.peak_import_kw = grid_kw;
            }
            let export_kw = (-grid_kw).max(0.0);
            if export_kw > self.peak_export_kw {
                self.peak_export_kw = export_kw;
            }
            self.rolling_peak.push(grid_kw);
        }

        for (name, slot) in &self.plan.end_use_columns {
            if let Some(value_kw) = value_at_slot(row, *slot) {
                if let Some(sum_kwh) = self.energy_by_end_use.get_mut(name) {
                    *sum_kwh += value_kw * self.timestep_h;
                }
                if let Some(peak_kw) = self.peak_by_end_use.get_mut(name)
                    && value_kw > *peak_kw
                {
                    *peak_kw = value_kw;
                }
            } else {
                self.nan_step_count += 1;
            }
        }

        if let Some(inputs) = &self.plan.setpoints {
            let heating_c = value_at_slot(row, inputs.heating_setpoint_slot);
            let cooling_c = value_at_slot(row, inputs.cooling_setpoint_slot);
            let deadband_c = match &inputs.deadband {
                DeadbandSource::Constant(value) => *value,
                DeadbandSource::Column(slot) => value_at_slot(row, *slot).unwrap_or(0.0),
            };

            if let (Some(heating_c), Some(cooling_c)) = (heating_c, cooling_c) {
                if deadband_c.is_finite() && deadband_c >= 0.0 {
                    let lower_bound_c = heating_c - deadband_c / 2.0;
                    let upper_bound_c = cooling_c + deadband_c / 2.0;
                    let mut all_inside = true;
                    let mut any_outside = false;
                    for &zone_slot in &inputs.zone_temperature_slots {
                        let Some(zone_c) = value_at_slot(row, zone_slot) else {
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

                    if any_outside && self.hvac_at_capacity(row) {
                        self.unmet_step_count += 1.0;
                    }
                } else {
                    self.setpoint_missing_row_count += 1;
                }
            } else {
                self.setpoint_missing_row_count += 1;
            }
        }

        // Envelope component loads [W → Wh via timestep_h].
        if let Some(slot) = self.plan.window_solar_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_window_solar_wh += v * self.timestep_h;
        }
        if let Some(slot) = self.plan.infiltration_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_infiltration_wh += v * self.timestep_h;
        }
        if let Some(slot) = self.plan.interior_lwr_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_interior_lwr_wh += v * self.timestep_h;
        }
        if let Some(slot) = self.plan.internal_gains_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_internal_gains_wh += v * self.timestep_h;
        }
        if let Some(slot) = self.plan.opaque_conduction_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_opaque_conduction_wh += v * self.timestep_h;
        }
        if let Some(slot) = self.plan.window_conduction_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_window_conduction_wh += v * self.timestep_h;
        }
        if let Some(slot) = self.plan.internal_mass_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_internal_mass_wh += v * self.timestep_h;
        }
        if let Some(slot) = self.plan.forced_ventilation_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_ventilation_wh += v * self.timestep_h;
        }
        if let Some(slot) = self.plan.natural_ventilation_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_ventilation_wh += v * self.timestep_h;
        }
        if let Some(slot) = self.plan.duct_loss_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_duct_loss_wh += v * self.timestep_h;
        }

        // HVAC thermal delivered accumulation for COP and envelope loads.
        if let Some(slot) = self.plan.hvac_heating_delivered_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_hvac_heating_wh += v * self.timestep_h;
        }
        if let Some(slot) = self.plan.hvac_cooling_delivered_w_slot
            && let Some(v) = value_at_slot(row, slot)
        {
            self.envelope_hvac_cooling_wh += v * self.timestep_h;
        }

        // HVAC electric power accumulation for COP.
        if let Some(slot) = self.plan.hvac_heating_kw_slot
            && let Some(kw) = value_at_slot(row, slot)
            && kw > 0.0
        {
            self.hvac_heating_electric_wh += kw * 1000.0 * self.timestep_h;
        }
        if let Some(slot) = self.plan.hvac_cooling_kw_slot
            && let Some(kw) = value_at_slot(row, slot)
            && kw > 0.0
        {
            self.hvac_cooling_electric_wh += kw * 1000.0 * self.timestep_h;
        }

        // Battery energy tracking for round-trip efficiency.
        if let Some(slot) = self.plan.battery_kw_slot
            && let Some(kw) = value_at_slot(row, slot)
        {
            if kw > 0.0 {
                self.battery_energy_in_kwh += kw * self.timestep_h;
            } else if kw < 0.0 {
                self.battery_energy_out_kwh += kw.abs() * self.timestep_h;
            }
        }

        // A NaN in any accumulator means a NaN power value contaminated the
        // totals: a typed error in every build profile.
        if self.total_electric_energy_kwh.is_nan() {
            return Err(MetricsError::NonFiniteAccumulator(
                "total_electric_energy_kwh".to_string(),
            ));
        }
        if self.total_consumption_kwh.is_nan() {
            return Err(MetricsError::NonFiniteAccumulator(
                "total_consumption_kwh".to_string(),
            ));
        }
        if self.total_gas_energy_therms.is_nan() {
            return Err(MetricsError::NonFiniteAccumulator(
                "total_gas_energy_therms".to_string(),
            ));
        }
        if self.has_envelope_columns {
            if self.envelope_hvac_heating_wh.is_nan() {
                return Err(MetricsError::NonFiniteAccumulator(
                    "envelope_hvac_heating_wh".to_string(),
                ));
            }
            if self.envelope_hvac_cooling_wh.is_nan() {
                return Err(MetricsError::NonFiniteAccumulator(
                    "envelope_hvac_cooling_wh".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Finalize and return all derived metrics.
    ///
    /// # Errors
    /// A non-finite accumulator or peak at finish time is a typed error in
    /// every build profile: NaN/Inf propagation early prevents downstream
    /// corruption of visualisation and RL environment observation pipelines.
    pub fn finish(self) -> Result<FullSimulationMetrics, MetricsError> {
        #[cfg(feature = "observe")]
        self.log_end_use_diagnostics();
        let mut peak_by_end_use = self.peak_by_end_use;
        for value in peak_by_end_use.values_mut() {
            if *value == f64::NEG_INFINITY {
                *value = 0.0;
            }
        }

        let renewable_energy_fraction = self.plan.pv_power_slot.map(|_| {
            if self.total_consumption_kwh <= 0.0 {
                0.0
            } else {
                // Fraction of the gross load covered by PV. Not clamped: a
                // net-positive home legitimately covers more than 100 % of
                // its load, and clamping would report exactly that as 1.0.
                self.total_pv_generation_kwh_abs / self.total_consumption_kwh
            }
        });

        let rows_with_partial_setpoint_data_fraction = self.plan.setpoints.as_ref().map(|_| {
            if self.total_row_count > 0 {
                self.setpoint_missing_row_count as f64 / self.total_row_count as f64
            } else {
                0.0
            }
        });

        // Compute actual simulation duration and coverage.
        let actual_duration_h = self.total_row_count as f64 * self.timestep_h;

        // ASHRAE HoF 2021 Ch.15: standard (non-leap) year = 365 d = 8760 h;
        // leap year = 366 d = 8784 h. The calendar year is the run's start
        // time in-run, the table's first Time value for the table feed.
        let year = self.coverage_year;
        let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);

        // Coverage: determined by accumulated row-hours vs a full calendar year.
        let coverage = if actual_duration_h < 8759.0 {
            SimulationCoverage::PartialYear
        } else if actual_duration_h > 8784.0 {
            SimulationCoverage::MultiYear
        } else if is_leap && actual_duration_h >= 8759.0 {
            SimulationCoverage::LeapYear
        } else {
            SimulationCoverage::FullYear
        };

        // The result already carries the accumulated duration and its
        // coverage (`simulation_duration_hours`, `coverage`): a run that
        // finished before its configured end legitimately has fewer rows
        // (a stepped run stopped early, a skipped Feb 29), so no
        // row-hours-versus-config.duration check fires here.

        // Warn on non-full-year simulations.
        match coverage {
            SimulationCoverage::PartialYear => {
                tracing::warn!(
                    actual_duration_h = actual_duration_h,
                    "simulation covers a partial year; energy totals do not represent a full calendar year"
                );
            }
            SimulationCoverage::MultiYear => {
                tracing::warn!(
                    actual_duration_h = actual_duration_h,
                    "simulation covers multiple years; energy totals aggregate more than one calendar year"
                );
            }
            SimulationCoverage::FullYear | SimulationCoverage::LeapYear => {}
        }

        // Invariant: no accumulator may contain NaN or Inf at finish time.
        // Catching NaN/Inf propagation early prevents downstream corruption
        // of visualisation and RL environment observation pipelines: a typed
        // error in every build profile.
        if self.total_electric_energy_kwh.is_nan() || !self.total_electric_energy_kwh.is_finite() {
            return Err(MetricsError::NonFiniteAccumulator(
                "total_electric_energy_kwh".to_string(),
            ));
        }
        if self.total_consumption_kwh.is_nan() || !self.total_consumption_kwh.is_finite() {
            return Err(MetricsError::NonFiniteAccumulator(
                "total_consumption_kwh".to_string(),
            ));
        }
        if self.total_gas_energy_therms.is_nan() || !self.total_gas_energy_therms.is_finite() {
            return Err(MetricsError::NonFiniteAccumulator(
                "total_gas_energy_therms".to_string(),
            ));
        }
        if self.peak_import_kw.is_nan() || !self.peak_import_kw.is_finite() {
            return Err(MetricsError::NonFiniteAccumulator(
                "peak_import_kw".to_string(),
            ));
        }
        if self.peak_export_kw.is_nan() || !self.peak_export_kw.is_finite() {
            return Err(MetricsError::NonFiniteAccumulator(
                "peak_export_kw".to_string(),
            ));
        }
        if self.has_envelope_columns {
            if self.envelope_hvac_heating_wh.is_nan() || !self.envelope_hvac_heating_wh.is_finite()
            {
                return Err(MetricsError::NonFiniteAccumulator(
                    "envelope_hvac_heating_wh".to_string(),
                ));
            }
            if self.envelope_hvac_cooling_wh.is_nan() || !self.envelope_hvac_cooling_wh.is_finite()
            {
                return Err(MetricsError::NonFiniteAccumulator(
                    "envelope_hvac_cooling_wh".to_string(),
                ));
            }
        }

        // Normalize leap-year totals to standard 8760 h year
        // (ASHRAE HoF 2021 Ch.15). Applies to both electric and gas
        // energy so that the combined total is on a consistent time base.
        let leap_factor = if coverage == SimulationCoverage::LeapYear {
            HOURS_PER_YEAR / actual_duration_h
        } else {
            1.0
        };
        let total_electric_energy_kwh = self.total_electric_energy_kwh * leap_factor;
        let total_consumption_kwh = self.total_consumption_kwh * leap_factor;
        let total_pv_generation_kwh = self.total_pv_generation_kwh_abs * leap_factor;
        let energy_by_end_use = self
            .energy_by_end_use
            .into_iter()
            .map(|(k, v)| (k, v * leap_factor))
            .collect();

        let gas_energy = self.plan.total_gas_power_slot.map(|_| {
            let therms = self.total_gas_energy_therms * leap_factor;
            GasEnergyMetrics {
                total_therms: therms,
                total_kwh_equivalent: energy_therms_to_kwh(therms),
            }
        });

        let nan_step_count = self.nan_step_count;
        let metrics_reliability = if nan_step_count > 0 {
            Reliability::Degraded
        } else {
            Reliability::Reliable
        };

        #[cfg(feature = "observe")]
        if nan_step_count > 0 {
            tracing::warn!(
                nan_step_count,
                "non-finite power values were detected and skipped during metric accumulation; \
                 check source simulation output for NaN/Inf telemetry"
            );
        }

        Ok(FullSimulationMetrics {
            metrics: SimulationMetrics {
                total_energy_kwh: TotalEnergyKwh {
                    net_energy_kwh: total_electric_energy_kwh,
                    gross_consumption_kwh: total_consumption_kwh,
                    gross_pv_generation_kwh: total_pv_generation_kwh,
                    per_end_use: energy_by_end_use,
                    duration_hours: actual_duration_h,
                },
                peak_power_kw: PeakPowerKw {
                    per_end_use: peak_by_end_use,
                    rolling: self.rolling_peak.finish(),
                },
                comfort_hours: self
                    .plan
                    .setpoints
                    .as_ref()
                    .map(|_| self.comfort_step_count * self.timestep_h),
                unmet_load_hours: self
                    .plan
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
                        window_conduction_kwh: self.envelope_window_conduction_wh / 1000.0,
                        opaque_conduction_kwh: self.envelope_opaque_conduction_wh / 1000.0,
                        internal_mass_kwh: self.envelope_internal_mass_wh / 1000.0,
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
                    // COP = delivered thermal / electric input. The delivered
                    // columns ("HVAC * Delivered (W)") only exist at high
                    // output verbosity; when absent the thermal numerator is
                    // unknown (not zero), so COP must be None — reporting
                    // 0.0 would misstate a working heat pump's efficiency.
                    hvac_heating_cop: if self.plan.hvac_heating_delivered_w_slot.is_some()
                        && self.hvac_heating_electric_wh > EPSILON
                    {
                        // Thermal output from envelope gains; electric input from telemetry column.
                        let thermal = self.envelope_hvac_heating_wh.max(0.0);
                        Some(thermal / self.hvac_heating_electric_wh)
                    } else {
                        None
                    },
                    hvac_cooling_cop: if self.plan.hvac_cooling_delivered_w_slot.is_some()
                        && self.hvac_cooling_electric_wh > EPSILON
                    {
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
                rows_with_partial_setpoint_data_fraction,
                simulation_duration_hours: actual_duration_h,
                coverage,
                nan_step_count,
                metrics_reliability,
            },
            gas_energy,
        })
    }

    /// One end-use diagnostics log per run, at finish (the per-batch
    /// boundary log of the flushed-batch feed is gone with it).
    #[cfg(feature = "observe")]
    fn log_end_use_diagnostics(&self) {
        let mut parts: Vec<String> = Vec::new();
        for (key, val) in &self.energy_by_end_use {
            parts.push(format!("{key}={val:.3}kWh"));
        }
        for (key, count) in &self.plan.end_use_equipment_counts {
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
            "end-use metrics at run finish"
        );
    }
}

/// Renders naive local wall-clock milliseconds as the wall clock they
/// carry: the reading the `Time` column's values name in errors.
fn naive_local_clock(local_ms: i64) -> String {
    DateTime::from_timestamp_millis(local_ms)
        .map(|instant| instant.naive_utc().to_string())
        .unwrap_or_else(|| format!("{local_ms} ms"))
}

/// Computes the run metrics from any results table: the same accumulator
/// the run feeds per step, fed from the table instead.
///
/// The table is a `Timestep` table: a `Time` column of naive local
/// wall-clock milliseconds and `Float64` columns, every consecutive pair
/// of `Time` values exactly `time_res` apart and no `Step Count` column.
/// A reduced table passed with the step resolution is an error naming the
/// first offending pair of `Time` values (or the `Step Count` column), so
/// an `Hourly` table passed with `time_res = 900` is an error, not wrong
/// metrics. The columns resolve through the catalogue: a column whose
/// Arrow type contradicts its declaration is an error naming it. An empty
/// table is an error naming it.
///
/// What the run's calculator reads from its stored config comes from the
/// table: the year of the first `Time` value decides the leap-year
/// coverage, the duration is the row count times `time_res`, and
/// `setpoint_deadband_c` is the config field's value. A metrics input
/// variable absent from the table leaves the metric it feeds `None`, by
/// the same rule as in-run (the total electric power column is required).
/// A table the run itself recorded therefore scores the run's metrics on
/// bits.
///
/// # Errors
///
/// [`MetricsError::EmptyTable`] for a table with no rows;
/// [`MetricsError::NonTimestepTable`] for a `Step Count` column, a
/// missing `Time` column, a null `Time` value, a `Time` value outside
/// the calendar range, or a consecutive pair not exactly `time_res`
/// apart;
/// [`MetricsError::ColumnTypeMismatch`] for a column contradicting its
/// catalogue declaration; [`MetricsError::InconsistentBatches`] for
/// batches that do not share one schema;
/// [`MetricsError::InvalidTimeResolution`] for a `time_res` that is not a
/// whole number of positive seconds; and the plan build's and the
/// accumulator's own errors (a missing required column, a non-finite
/// accumulator).
pub fn metrics_from_table(
    table: &[RecordBatch],
    catalogue: &Catalogue,
    time_res: Duration,
    setpoint_deadband_c: Option<f64>,
) -> Result<FullSimulationMetrics, MetricsError> {
    // The step resolution: a whole number of positive seconds, the rule
    // the config's own validation runs.
    let time_res_ms = time_res.num_milliseconds();
    if time_res_ms <= 0 || time_res_ms % 1000 != 0 {
        return Err(MetricsError::InvalidTimeResolution);
    }
    let time_res_secs =
        u32::try_from(time_res.num_seconds()).map_err(|_| MetricsError::InvalidTimeResolution)?;

    let total_rows: usize = table.iter().map(|batch| batch.num_rows()).sum();
    if total_rows == 0 {
        return Err(MetricsError::EmptyTable);
    }

    let time_label = time_field();
    let schema = table[0].schema();
    // One table, one layout: every batch carries the first batch's fields,
    // in order, so one schema check covers the table.
    for (index, batch) in table.iter().enumerate() {
        let batch_schema = batch.schema();
        let fields = batch_schema.fields();
        if fields != schema.fields() {
            let first_fields = schema.fields();
            let column = (0..fields.len().max(first_fields.len()))
                .find(|&i| fields.get(i) != first_fields.get(i))
                .and_then(|i| fields.get(i).or(first_fields.get(i)))
                .map(|field| field.name().clone())
                .unwrap_or_default();
            return Err(MetricsError::InconsistentBatches {
                batch: index,
                column,
            });
        }
    }

    // No `Step Count` column: a reduced table's rows are periods, not
    // steps, and its step counts are not a timestep stream.
    if schema
        .fields()
        .iter()
        .any(|field| field.name() == STEP_COUNT_COLUMN)
    {
        return Err(MetricsError::NonTimestepTable(format!(
            "the table carries a `{STEP_COUNT_COLUMN}` column: a reduced table's rows are \
             periods, not steps"
        )));
    }

    // Every column resolves through the catalogue: a column whose Arrow
    // type contradicts its declaration is an error naming it. A column the
    // catalogue declares nothing for has nothing to contradict, and no
    // metrics input reads it.
    for field in schema.fields() {
        let declared: Option<DataType> = if field.name() == time_label.name() {
            Some(time_label.data_type().clone())
        } else {
            catalogue
                .resolve(field.name())
                .map(|def| variable_field(def).data_type().clone())
        };
        let Some(declared) = declared else {
            continue;
        };
        if field.data_type() != &declared {
            return Err(MetricsError::ColumnTypeMismatch {
                column: field.name().clone(),
                expected: declared.to_string(),
                found: field.data_type().to_string(),
            });
        }
    }

    // The `Time` column, wherever the schema puts it.
    let time_index = schema
        .fields()
        .iter()
        .position(|field| field.name() == time_label.name())
        .ok_or_else(|| {
            MetricsError::NonTimestepTable("the table carries no Time column".to_string())
        })?;

    // Consecutive `Time` values differ by exactly `time_res`, across the
    // batch boundaries too: the first offending pair names the mismatch,
    // a null slot names itself (the value buffer's raw 0-ms slot is not
    // the row's reading).
    let mut first_time_ms: Option<i64> = None;
    let mut previous_ms: Option<i64> = None;
    let mut rows_before = 0usize;
    for batch in table {
        let times = batch
            .column(time_index)
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .ok_or_else(|| MetricsError::ColumnTypeMismatch {
                column: time_label.name().clone(),
                expected: time_label.data_type().to_string(),
                found: batch.schema().field(time_index).data_type().to_string(),
            })?;
        for row in 0..batch.num_rows() {
            if times.is_null(row) {
                return Err(MetricsError::NonTimestepTable(format!(
                    "a null Time value at row {}",
                    rows_before + row
                )));
            }
            let current = times.value(row);
            if first_time_ms.is_none() {
                first_time_ms = Some(current);
            }
            if let Some(previous) = previous_ms
                && current - previous != time_res_ms
            {
                return Err(MetricsError::NonTimestepTable(format!(
                    "consecutive Time values {} and {} differ by {} ms, not the time \
                     resolution's {time_res_ms} ms",
                    naive_local_clock(previous),
                    naive_local_clock(current),
                    current - previous,
                )));
            }
            previous_ms = Some(current);
        }
        rows_before += batch.num_rows();
    }
    let first_time_ms = first_time_ms.ok_or(MetricsError::EmptyTable)?;

    // The calendar year the coverage reads: the first `Time` value's year.
    let coverage_year = DateTime::from_timestamp_millis(first_time_ms)
        .ok_or_else(|| {
            MetricsError::NonTimestepTable(format!(
                "the first Time value {first_time_ms} ms is outside the calendar range"
            ))
        })?
        .naive_utc()
        .year();

    let value_indices: Vec<usize> = (0..schema.fields().len())
        .filter(|index| *index != time_index)
        .collect();
    let names: Vec<String> = value_indices
        .iter()
        .map(|&index| schema.field(index).name().clone())
        .collect();
    let plan = MetricsInputPlan::build_from_names(&names, setpoint_deadband_c)?;
    let mut calculator = MetricsCalculator::for_coverage_year(plan, time_res_secs, coverage_year)?;

    // Every row, in table order, through the accumulator the run feeds
    // per step: the same aggregation, the same values.
    let mut row_buf: Vec<f64> = Vec::with_capacity(names.len());
    for batch in table {
        let mut value_columns = Vec::with_capacity(value_indices.len());
        for (slot, &index) in value_indices.iter().enumerate() {
            let column = batch
                .column(index)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| MetricsError::ColumnTypeMismatch {
                    column: names[slot].clone(),
                    expected: DataType::Float64.to_string(),
                    found: batch.schema().field(index).data_type().to_string(),
                })?;
            value_columns.push(column);
        }
        for row in 0..batch.num_rows() {
            row_buf.clear();
            for column in &value_columns {
                // A null carries no value: NaN routes it through the
                // accumulator's non-finite guard, the skip a non-finite
                // power value gets in-run. The engine's own tables carry
                // no nulls.
                row_buf.push(if column.is_null(row) {
                    f64::NAN
                } else {
                    column.value(row)
                });
            }
            calculator.accumulate_row(&row_buf)?;
        }
    }
    calculator.finish()
}

fn discover_end_use_slots(
    available: &[String],
    total_electric_power_slot: usize,
    grid_power_slot: usize,
) -> Result<Vec<(String, usize)>, MetricsError> {
    let mut columns = Vec::new();
    for (slot, name) in available.iter().enumerate() {
        if slot == total_electric_power_slot || slot == grid_power_slot {
            continue;
        }
        // Match end-use aggregate columns via the central naming API.
        // Columns use the form "{EndUse Display Name} End Use Electric Power (kW)".
        if let Some(end_use_key) =
            crate::output::columns::parse_end_use_electric_power_column_key(name)
        {
            columns.push((end_use_key, slot));
        }
    }
    Ok(columns)
}

/// Returns true if the producer surface carries at least one per-equipment
/// electric power name that maps to the given `EndUse` category.
///
/// Used to gate the missing-aggregate-column error on the actual presence of
/// equipment for that end-use category.
#[cfg_attr(not(feature = "observe"), allow(dead_code))]
pub(crate) fn names_have_equipment_for(available: &[String], target: &hares_types::EndUse) -> bool {
    for name in available {
        if let Some(name) = name.as_str().strip_suffix(" Electric Power (kW)") {
            let prefix = name.trim();
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

/// Derives per-end-use equipment counts from the producer surface's names.
///
/// Counts each per-equipment electric power column whose name maps to a
/// recognised EndUse category, skipping aggregate columns (whose prefix is
/// an EndUse display name rather than an equipment instance name).
#[cfg(feature = "observe")]
fn compute_equipment_counts_from_names(available: &[String]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for name in available {
        let Some(prefix) = name.strip_suffix(ELECTRIC_POWER_SUFFIX) else {
            continue;
        };
        let prefix = prefix.trim();
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
    counts
}

/// The producer surface's position of a required metrics column.
fn required_slot(available: &[String], name: &str) -> Result<usize, MetricsError> {
    available
        .iter()
        .position(|candidate| candidate == name)
        .ok_or_else(|| MetricsError::MissingRequiredColumn(name.to_owned()))
}

fn optional_slot(available: &[String], candidates: &[&str]) -> Result<Option<usize>, MetricsError> {
    for name in candidates {
        if let Some(slot) = available.iter().position(|candidate| candidate == name) {
            return Ok(Some(slot));
        }
    }
    Ok(None)
}

fn discover_setpoint_inputs(
    available: &[String],
    config_deadband: Option<f64>,
) -> Result<Option<SetpointInputs>, MetricsError> {
    let heating_setpoint_slot = optional_slot(available, &["HVAC Heating Setpoint (C)"])?;
    let cooling_setpoint_slot = optional_slot(available, &["HVAC Cooling Setpoint (C)"])?;

    let (Some(heating_setpoint_slot), Some(cooling_setpoint_slot)) =
        (heating_setpoint_slot, cooling_setpoint_slot)
    else {
        return Ok(None);
    };

    let zone_temperature_slots = discover_conditioned_zone_temperatures(available)?;
    if zone_temperature_slots.is_empty() {
        return Ok(None);
    }

    let deadband = if let Some(deadband_c) = config_deadband {
        DeadbandSource::Constant(deadband_c)
    } else if let Some(slot) = optional_slot(available, &[SETPOINT_DEADBAND_C, DEADBAND_C])? {
        DeadbandSource::Column(slot)
    } else {
        return Ok(None);
    };

    Ok(Some(SetpointInputs {
        heating_setpoint_slot,
        cooling_setpoint_slot,
        zone_temperature_slots,
        deadband,
    }))
}

fn discover_conditioned_zone_temperatures(
    available: &[String],
) -> Result<Vec<usize>, MetricsError> {
    // OCHRE convention: "Temperature - Indoor (C)", "Temperature - {zone} (C)"
    let mut conditioned = Vec::new();
    for (slot, name) in available.iter().enumerate() {
        if name == "Temperature - Indoor (C)" {
            conditioned.push(slot);
        }
    }
    if !conditioned.is_empty() {
        return Ok(conditioned);
    }

    // Fallback: any "Temperature - {zone} (C)" column.
    let mut generic = Vec::new();
    for (slot, name) in available.iter().enumerate() {
        if name.starts_with("Temperature - ") && name.ends_with(" (C)") {
            generic.push(slot);
        }
    }
    Ok(generic)
}

fn discover_hvac_capacity_pairs(available: &[String]) -> Result<Vec<(usize, usize)>, MetricsError> {
    let mut pairs = Vec::new();
    // Try heating pair.
    if let (Some(output_slot), Some(capacity_slot)) = (
        optional_slot(available, &["HVAC Heating Delivered (W)"])?,
        optional_slot(available, &[tk::HVAC_HEATING_CAPACITY_W])?,
    ) {
        pairs.push((output_slot, capacity_slot));
    }
    // Try cooling pair.
    if let (Some(output_slot), Some(capacity_slot)) = (
        optional_slot(available, &["HVAC Cooling Delivered (W)"])?,
        optional_slot(available, &[tk::HVAC_COOLING_CAPACITY_W])?,
    ) {
        pairs.push((output_slot, capacity_slot));
    }
    Ok(pairs)
}

/// Relative tolerance for HVAC capacity comparison. Avoids false negatives
/// at high wattages where the absolute difference may exceed a tiny epsilon.
/// The `max(1.0)` denominator clamp means this transitions to a near-absolute
/// tolerance (~1e-6 W) below 1 W capacity, which is acceptable since sub-1W
/// HVAC capacities are unphysical.
const HVAC_CAPACITY_REL_TOL: f64 = 1e-6;

/// Read a value from the producer row, returning `Some` for finite values
/// and `None` for non-finite (NaN/Inf) values.
fn value_at_slot(row: &[f64], slot: usize) -> Option<f64> {
    let v = row[slot];
    v.is_finite().then_some(v)
}

/// A peak value as its snapshot form: `None` for a non-finite value (the
/// `NEG_INFINITY` sentinel an end use holds before its first finite
/// reading), `Some` for a finite one.
fn peak_finite(peak: f64) -> Option<f64> {
    peak.is_finite().then_some(peak)
}

/// The peak value a snapshot's form restores to: the `NEG_INFINITY`
/// sentinel back from `None`, the finite value from `Some`.
fn peak_non_finite_or(peak: Option<f64>) -> f64 {
    peak.unwrap_or(f64::NEG_INFINITY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::{ArrayRef, Float64Array},
        datatypes::{DataType, Field, Schema},
        record_batch::RecordBatch,
    };
    use chrono::{Duration, FixedOffset, TimeZone};
    use std::sync::Arc;

    /// The calculator for a test schema: the input plan built from the
    /// schema's own value-column names (a leading `Time` column skipped,
    /// the union-schema convention), then fed rows extracted from batches.
    fn calc_for(
        schema: &Schema,
        time_res_secs: u32,
        config: &SimulationConfig,
    ) -> MetricsCalculator {
        let plan = plan_for(schema, config);
        MetricsCalculator::new(plan, time_res_secs, config).expect("calculator builds")
    }

    fn plan_for(schema: &Schema, config: &SimulationConfig) -> MetricsInputPlan {
        let names: Vec<String> = schema
            .fields()
            .iter()
            .skip(usize::from(
                schema.fields().first().is_some_and(|f| f.name() == "Time"),
            ))
            .map(|f| f.name().clone())
            .collect();
        MetricsInputPlan::build(&names, config).expect("metrics plan builds")
    }

    /// Feeds every row of the batch, in order: the same values the
    /// producer row carried, one `accumulate_row` per row.
    fn accumulate(calc: &mut MetricsCalculator, batch: &RecordBatch) -> Result<(), MetricsError> {
        for r in 0..batch.num_rows() {
            let row: Vec<f64> = batch
                .columns()
                .iter()
                .map(|c| {
                    c.as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("f64 column")
                        .value(r)
                })
                .collect();
            calc.accumulate_row(&row)?;
        }
        Ok(())
    }

    fn test_config(deadband: Option<f64>) -> SimulationConfig {
        SimulationConfig {
            start_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .unwrap(),
            duration: Duration::hours(1),
            time_res: Duration::hours(1),
            reports: Vec::new(),
            diagnostics: false,
            metrics: false,
            output_chunk_size: 16,
            master_seed: 0,
            setpoint_deadband_c: deadband,
            civil_timezone: None,
            site_location: crate::SiteLocationOverride::default(),
            max_consecutive_step_failures: crate::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: crate::HealthPolicy::default(),
        }
    }

    fn test_config_full_year(year: i32, deadband: Option<f64>) -> SimulationConfig {
        // ASHRAE HoF 2021 Ch.15: non-leap year = 8760 h, leap year = 8784 h.
        let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let total_hours: i64 = if is_leap {
            8_784
        } else {
            HOURS_PER_YEAR as i64
        };
        SimulationConfig {
            start_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(year, 1, 1, 0, 0, 0)
                .single()
                .unwrap(),
            duration: Duration::hours(total_hours),
            time_res: Duration::hours(1),
            reports: Vec::new(),
            diagnostics: false,
            metrics: false,
            output_chunk_size: 16,
            master_seed: 0,
            setpoint_deadband_c: deadband,
            civil_timezone: None,
            site_location: crate::SiteLocationOverride::default(),
            max_consecutive_step_failures: crate::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: crate::HealthPolicy::default(),
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
    fn total_energy_matches_accumulated_rows() {
        // 2026 is not a leap year → full year = 8760 hours.
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating End Use Electric Power (kW)",
        ]);
        let config = test_config_full_year(2026, None);
        let mut calc = calc_for(&schema, 3600, &config);

        let rows = 8_760;
        let batch = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0; rows]),
            ("HVAC Heating End Use Electric Power (kW)", vec![1.0; rows]),
        ]);
        accumulate(&mut calc, &batch).unwrap();
        let metrics = calc.finish().unwrap();

        assert!((metrics.total_energy_kwh.net_energy_kwh - 8_760.0).abs() < 1e-9);
        assert!((metrics.total_energy_kwh.duration_hours - 8_760.0).abs() < 1e-9);
        assert_eq!(metrics.coverage, SimulationCoverage::FullYear);
        assert!((metrics.simulation_duration_hours - 8_760.0).abs() < 1e-9);
    }

    #[test]
    fn peak_power_uses_max_across_batches() {
        let schema =
            schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "EV End Use Electric Power (kW)"]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        let b1 = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![2.0, 4.0]),
            ("EV End Use Electric Power (kW)", vec![1.0, 3.5]),
        ]);
        let b2 = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![3.0, 5.0]),
            ("EV End Use Electric Power (kW)", vec![2.5, 6.2]),
        ]);
        accumulate(&mut calc, &b1).unwrap();
        accumulate(&mut calc, &b2).unwrap();
        let metrics = calc.finish().unwrap();

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
        let mut calc = calc_for(&schema, 3600, &test_config(Some(1.0)));

        let batch = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0, 1.0]),
            ("Temperature - Indoor (C)", vec![21.0, 21.4, 22.0]),
            ("HVAC Heating Setpoint (C)", vec![21.0, 21.0, 21.0]),
            ("HVAC Cooling Setpoint (C)", vec![22.0, 22.0, 22.0]),
            ("HVAC Heating Delivered (W)", vec![0.0, 0.0, 0.0]),
            (tk::HVAC_HEATING_CAPACITY_W, vec![4.0, 4.0, 4.0]),
        ]);
        accumulate(&mut calc, &batch).unwrap();
        let metrics = calc.finish().unwrap();

        assert_eq!(metrics.comfort_hours, Some(3.0));
        assert_eq!(metrics.unmet_load_hours, Some(0.0));
    }

    #[test]
    fn verbosity_zero_schema_returns_none_for_gated_metrics() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        accumulate(
            &mut calc,
            &build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![1.0, 2.0])]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        assert_eq!(metrics.comfort_hours, None);
        assert_eq!(metrics.unmet_load_hours, None);
    }

    #[test]
    fn renewable_fraction_uses_abs_for_negative_pv() {
        let schema =
            schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "PV End Use Electric Power (kW)"]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![4.0, 4.0]),
                ("PV End Use Electric Power (kW)", vec![-2.0, -1.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        // Gross load per row = net total + self-consumed PV generation:
        // (4+2) + (4+1) = 11 kWh; PV generated 3 kWh → fraction 3/11.
        // The old denominator (positive part of the net column, 8 kWh)
        // ignored self-consumed PV and understated the load.
        assert_eq!(metrics.renewable_energy_fraction, Some(3.0 / 11.0));
        // Identity: net = gross_consumption − gross_pv (PV-only export).
        assert!(
            (metrics.total_energy_kwh.net_energy_kwh
                - (metrics.total_energy_kwh.gross_consumption_kwh
                    - metrics.total_energy_kwh.gross_pv_generation_kwh))
                .abs()
                < 1e-9
        );
    }

    /// A net-positive home (exports more than it imports over the period)
    /// reports a renewable fraction above 1.0 — its PV covers more than
    /// 100 % of the load behind the meter. Clamping to 1.0 would erase
    /// exactly that signal. Pins the documented unclamped contract.
    #[test]
    fn renewable_fraction_exceeds_one_for_net_exporter() {
        let schema =
            schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "PV End Use Electric Power (kW)"]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        // Net meter −2 kW (exporting) while PV generates 5 kW: load behind
        // the meter is −2 + 5 = 3 kWh, PV is 5 kWh → fraction 5/3 > 1.0.
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![-2.0]),
                ("PV End Use Electric Power (kW)", vec![-5.0]),
            ]),
        )
        .unwrap();
        let m = calc.finish().unwrap();
        assert_eq!(m.renewable_energy_fraction, Some(5.0 / 3.0));
        assert!(m.renewable_energy_fraction.unwrap() > 1.0);
    }

    /// Randomized energy-identity property: for any sign pattern of net
    /// total and PV columns,
    ///   gross_consumption − gross_pv − net == −Σ min(total + pv_gen, 0)·dt
    /// exactly — the right-hand side is the documented battery/export caveat:
    /// zero for PV-only-export rows (load ≥ 0 after self-consumption), equal
    /// to clipped export otherwise. 256 deterministic cases (SplitMix64).
    #[test]
    fn energy_identity_holds_over_randomized_sign_patterns() {
        let mut state = 0xB5297A4D_u64;
        let mut next_f64 = move || {
            state = state.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^= z >> 31;
            (z >> 11) as f64 / (1u64 << 53) as f64
        };

        for case in 0..256 {
            let n_rows = 1 + (next_f64() * 24.0) as usize;
            // Net total spans export..heavy import; PV column negative when
            // generating (signed) with occasional NaN rows excluded by design.
            let total: Vec<f64> = (0..n_rows).map(|_| (next_f64() - 0.4) * 20.0).collect();
            let pv: Vec<f64> = (0..n_rows)
                .map(|_| {
                    if next_f64() < 0.5 {
                        -next_f64() * 10.0
                    } else {
                        0.0
                    }
                })
                .collect();

            let schema =
                schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "PV End Use Electric Power (kW)"]);
            let mut calc = calc_for(&schema, 3600, &test_config(None));
            accumulate(
                &mut calc,
                &build_batch(vec![
                    (TOTAL_ELECTRIC_POWER_KW, total.clone()),
                    ("PV End Use Electric Power (kW)", pv.clone()),
                ]),
            )
            .unwrap();
            let full = calc.finish().unwrap();
            let m = &full.total_energy_kwh;

            // Reference computation, straight from the documented semantics.
            let mut exp_net = 0.0;
            let mut exp_consumption = 0.0;
            let mut exp_pv = 0.0;
            for (&t, &p) in total.iter().zip(&pv) {
                let pv_gen = if p < 0.0 { p.abs() } else { 0.0 };
                exp_net += t;
                exp_consumption += (t + pv_gen).max(0.0);
                exp_pv += pv_gen;
            }
            let lhs = m.gross_consumption_kwh - m.gross_pv_generation_kwh - m.net_energy_kwh;
            let rhs = exp_consumption - exp_pv - exp_net;
            assert!(
                (lhs - rhs).abs() < 1e-9,
                "case {case}: identity residual {lhs:.6e} vs reference {rhs:.6e} \
                 (net={:.3}, consumption={:.3}, pv={:.3})",
                m.net_energy_kwh,
                m.gross_consumption_kwh,
                m.gross_pv_generation_kwh
            );
            // And the accumulators themselves match the reference exactly.
            assert!(
                (m.net_energy_kwh - exp_net).abs() < 1e-9,
                "case {case}: net"
            );
            assert!(
                (m.gross_consumption_kwh - exp_consumption).abs() < 1e-9,
                "case {case}: gross consumption"
            );
            assert!(
                (m.gross_pv_generation_kwh - exp_pv).abs() < 1e-9,
                "case {case}: gross pv"
            );
        }
    }

    /// The documented energy identity — `TotalEnergyKwh`: "net ==
    /// gross_consumption − gross_pv holds whenever PV is the only export
    /// source" — must survive a non-finite net-total row, an input class the
    /// calculator explicitly handles elsewhere (`guarded_power_value`
    /// returns None for it and `nan_step_count` counts it). On such a row
    /// the PV column can still carry real, finite generation: the hoisted
    /// per-row `pv_gen_kw` accumulates it while the net and
    /// gross-consumption paths skip the row, so the triple silently stops
    /// closing (and `renewable_energy_fraction` divides a numerator that
    /// counted the row by a denominator that did not).
    #[test]
    fn energy_identity_holds_across_non_finite_total_rows() {
        let schema =
            schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "PV End Use Electric Power (kW)"]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        // Row 0: meter total non-finite (sensor dropout), PV generating 2 kW.
        // Row 1: healthy import row, no PV. PV is the only export source.
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![f64::NAN, 4.0]),
                ("PV End Use Electric Power (kW)", vec![-2.0, 0.0]),
            ]),
        )
        .unwrap();
        let m = &calc.finish().unwrap().total_energy_kwh;
        let residual = m.net_energy_kwh - (m.gross_consumption_kwh - m.gross_pv_generation_kwh);
        assert!(
            residual.abs() < 1e-9,
            "documented identity net == gross_consumption − gross_pv violated by \
             {residual:.6} kWh (net={:.3}, consumption={:.3}, pv={:.3}): the PV \
             accumulator counted generation on a row whose non-finite total made \
             the net and gross-consumption accumulators skip it",
            m.net_energy_kwh,
            m.gross_consumption_kwh,
            m.gross_pv_generation_kwh
        );
    }

    #[test]
    fn renewable_fraction_none_without_pv_column() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let calc = calc_for(&schema, 3600, &test_config(None));
        let metrics = calc.finish().unwrap();
        assert_eq!(metrics.renewable_energy_fraction, None);
    }

    #[test]
    fn peak_export_is_zero_when_grid_never_negative() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, GRID_ELECTRIC_POWER_KW]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 2.0, 3.0]),
                (GRID_ELECTRIC_POWER_KW, vec![0.0, 2.0, 1.5]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();
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
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0, 1.0]),
                ("Temperature - Indoor (C)", vec![24.0, 22.0, 24.0]),
                ("HVAC Heating Setpoint (C)", vec![21.0, 21.0, 21.0]),
                ("HVAC Cooling Setpoint (C)", vec![22.0, 22.0, 22.0]),
                ("Setpoint Deadband (C)", vec![1.0, 1.0, 1.0]),
                ("HVAC Heating Delivered (W)", vec![3.0, 2.0, 3.0]),
                (tk::HVAC_HEATING_CAPACITY_W, vec![3.0, 3.0, 3.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        assert_eq!(metrics.unmet_load_hours, Some(2.0));
    }

    #[test]
    fn constructor_requires_total_electric_power_column() {
        let schema = schema_from_columns(&["HVAC Heating End Use Electric Power (kW)"]);
        let names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
        let err = MetricsInputPlan::build(&names, &test_config(None)).unwrap_err();
        assert_eq!(
            err,
            MetricsError::MissingRequiredColumn(TOTAL_ELECTRIC_POWER_KW.to_string())
        );
    }

    #[test]
    fn gas_energy_tracked_when_gas_column_present() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, TOTAL_GAS_POWER_THERMS]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0]),
                (TOTAL_GAS_POWER_THERMS, vec![2.0, 3.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        let gas = metrics
            .gas_energy
            .as_ref()
            .expect("gas energy should be present");
        assert!((gas.total_therms - 5.0).abs() < 1e-9);
        assert!((metrics.total_energy_kwh.net_energy_kwh - 2.0).abs() < 1e-9);
        let expected_combined = 2.0 + energy_therms_to_kwh(5.0);
        assert!((metrics.combined_total_energy_kwh() - expected_combined).abs() < 1e-6);
    }

    #[test]
    fn gas_energy_none_when_no_gas_column() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let calc = calc_for(&schema, 3600, &test_config(None));
        let metrics = calc.finish().unwrap();
        assert!(metrics.gas_energy.is_none());
        assert!((metrics.combined_total_energy_kwh() - 0.0).abs() < 1e-9);
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
            typed_overrides: serde_json::Map::new(),
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }];
        let schema = crate::output::build_schema_for_specs(&specs, 1, &[]);
        let result = plan_for(&schema, &test_config(None));
        // The plan carries the resolved slots: assert it built and the
        // calculator accepts it.
        let _calc = MetricsCalculator::new(result, 3600, &test_config(None))
            .expect("MetricsCalculator should accept a plan from build_schema output");
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
        let mut calc = calc_for(&schema, 3600, &test_config(Some(1.0)));

        let rows = 24;
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![2.0; rows]),
                (TOTAL_GAS_POWER_THERMS, vec![0.5; rows]),
                ("Temperature - Indoor (C)", vec![21.5; rows]),
                ("HVAC Heating Setpoint (C)", vec![21.0; rows]),
                ("HVAC Cooling Setpoint (C)", vec![23.0; rows]),
                ("HVAC Heating Delivered (W)", vec![1.0; rows]),
                (tk::HVAC_HEATING_CAPACITY_W, vec![5.0; rows]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        assert!(
            metrics.total_energy_kwh.net_energy_kwh > 0.0,
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
        let mut calc = calc_for(&schema, 3600, &test_config(Some(1.0)));

        // Row with output nearly equal to capacity at high wattage (50,000 W = 50 kW).
        // Difference = 0.00005 W → relative = 0.00005 / 50000 = 1e-9, well within 1e-6.
        let capacity = 50_000.0;
        let output = capacity - 0.00005;
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![1.0]),
                ("Temperature - Indoor (C)", vec![25.0]), // outside cooling setpoint
                ("HVAC Heating Setpoint (C)", vec![21.0]),
                ("HVAC Cooling Setpoint (C)", vec![22.0]),
                ("HVAC Heating Delivered (W)", vec![output]),
                (tk::HVAC_HEATING_CAPACITY_W, vec![capacity]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

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
            "HVAC Heating End Use Electric Power (kW)",
            "HVAC Heating Delivered (W)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // 10 kW thermal delivery, 2 kW electric → COP = 10000 Wh / 2000 Wh = 5.0
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![2.0, 2.0]),
                ("HVAC Heating End Use Electric Power (kW)", vec![2.0, 2.0]),
                ("HVAC Heating Delivered (W)", vec![10_000.0, 10_000.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

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

    /// When the schema lacks the "HVAC Heating Delivered (W)" column (only
    /// emitted at high output verbosity), the thermal numerator is unknown —
    /// COP must be None, never a misleading 0.0. Regression test for the
    /// verbosity-1 case where electric power exists but delivered heat does
    /// not.
    #[test]
    fn hvac_heating_cop_none_without_delivered_column() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating End Use Electric Power (kW)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![2.0, 2.0]),
                ("HVAC Heating End Use Electric Power (kW)", vec![2.0, 2.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();
        assert_eq!(
            metrics.metrics.efficiency.hvac_heating_cop, None,
            "COP must be None when delivered-heat column is absent (unknown thermal), not 0.0"
        );
    }

    #[test]
    fn hvac_cooling_cop_uses_abs_of_negative_delivery() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Cooling End Use Electric Power (kW)",
            "HVAC Cooling Delivered (W)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // -8000 W cooling (negative = heat removal), 2 kW electric → COP = 8000/2000 = 4.0
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![2.0]),
                ("HVAC Cooling End Use Electric Power (kW)", vec![2.0]),
                ("HVAC Cooling Delivered (W)", vec![-8_000.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

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
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![0.0]),
                ("HVAC Heating Delivered (W)", vec![5000.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        assert!(metrics.metrics.efficiency.hvac_heating_cop.is_none());
    }

    #[test]
    fn battery_round_trip_efficiency_computed() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Battery End Use Electric Power (kW)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // 10 kWh in, 9 kWh out → 90% round trip
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![10.0, -9.0]),
                ("Battery End Use Electric Power (kW)", vec![10.0, -9.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

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
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // 500 W forced + 300 W natural = 800 W → 800 Wh → 0.8 kWh for 1 hour
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![1.0]),
                ("Forced Ventilation Heat Gain - Indoor (W)", vec![500.0]),
                ("Natural Ventilation Heat Gain - Indoor (W)", vec![300.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

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
            "Window Heat Gain - Indoor (W)",
            "Internal Mass Heat Gain - Indoor (W)",
            "Interior LWR Exchange - Indoor (W)",
            "Infiltration Heat Gain - Indoor (W)",
            "Forced Ventilation Heat Gain - Indoor (W)",
            "Internal Heat Gain - Indoor (W)",
            "Duct Loss Heat Gain - Indoor (W)",
            "HVAC Heating Delivered (W)",
            "HVAC Cooling Delivered (W)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![1.0]),
                ("Window Transmitted Solar Gain (W)", vec![1000.0]),
                ("Opaque Surface Heat Gain - Indoor (W)", vec![200.0]),
                ("Window Heat Gain - Indoor (W)", vec![150.0]),
                ("Internal Mass Heat Gain - Indoor (W)", vec![25.0]),
                ("Interior LWR Exchange - Indoor (W)", vec![50.0]),
                ("Infiltration Heat Gain - Indoor (W)", vec![-100.0]),
                ("Forced Ventilation Heat Gain - Indoor (W)", vec![-50.0]),
                ("Internal Heat Gain - Indoor (W)", vec![300.0]),
                ("Duct Loss Heat Gain - Indoor (W)", vec![75.0]),
                ("HVAC Heating Delivered (W)", vec![5000.0]),
                ("HVAC Cooling Delivered (W)", vec![-3000.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();
        let loads = metrics
            .metrics
            .envelope_loads_kwh
            .expect("envelope loads present");

        assert!((loads.window_solar_kwh - 1.0).abs() < 1e-9);
        assert!((loads.opaque_conduction_kwh - 0.2).abs() < 1e-9);
        assert!((loads.window_conduction_kwh - 0.15).abs() < 1e-9);
        assert!((loads.internal_mass_kwh - 0.025).abs() < 1e-9);
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
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        accumulate(
            &mut calc,
            &build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![1.0])]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();
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
            "HVAC Heating End Use Electric Power (kW)",
            "Battery End Use Electric Power (kW)",
        ]);
        let calc = calc_for(&schema, 3600, &test_config(None));
        let metrics = calc.finish().unwrap();

        // energy_by_end_use should have "hvac_heating" and "battery" keys
        // (from aggregate columns), NOT "ASHP Heater" or "Gas Furnace"
        // (from per-equipment columns).
        assert!(
            metrics
                .total_energy_kwh
                .per_end_use
                .contains_key("hvac_heating"),
            "per_end_use must contain key 'hvac_heating' from aggregate column"
        );
        assert!(
            metrics.total_energy_kwh.per_end_use.contains_key("battery"),
            "per_end_use must contain key 'battery' from aggregate column"
        );
        assert!(
            !metrics
                .total_energy_kwh
                .per_end_use
                .contains_key("ASHP Heater"),
            "per_end_use must NOT contain per-equipment key 'ASHP Heater'"
        );
        assert!(
            !metrics
                .total_energy_kwh
                .per_end_use
                .contains_key("Gas Furnace"),
            "per_end_use must NOT contain per-equipment key 'Gas Furnace'"
        );
    }

    /// Multiple HVAC_HEATING equipment power contributions are accumulated
    /// under the same `"hvac_heating"` end-use key.
    #[test]
    fn energy_by_end_use_aggregates_multiple_hvac_heating_equipment() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating End Use Electric Power (kW)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        // Simulate two hours at constant 3 kW
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![3.0, 3.0]),
                ("HVAC Heating End Use Electric Power (kW)", vec![3.0, 3.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();
        assert!(
            metrics.total_energy_kwh.per_end_use["hvac_heating"] > 0.0,
            "hvac_heating energy must be non-zero"
        );
        assert!(
            (metrics.total_energy_kwh.per_end_use["hvac_heating"] - 6.0).abs() < 1e-9,
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
            "HVAC Heating End Use Electric Power (kW)",
            "HVAC Heating Delivered (W)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![3.0, 3.0]),
                ("HVAC Heating End Use Electric Power (kW)", vec![2.0, 2.5]),
                ("HVAC Heating Delivered (W)", vec![6000.0, 7000.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();
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

    // ── Equipment-count derivation from names ──────────────────────────

    fn names_of(schema: &Schema) -> Vec<String> {
        schema.fields().iter().map(|f| f.name().clone()).collect()
    }

    /// `names_have_equipment_for` detects HVAC equipment columns in the
    /// producer surface and correctly returns false when none are present.
    #[test]
    fn names_have_equipment_for_detects_hvac_columns() {
        let hvac_names = names_of(&schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "ASHP Heater Electric Power (kW)",
            "HVAC Heating End Use Electric Power (kW)",
        ]));
        assert!(
            names_have_equipment_for(&hvac_names, &hares_types::EndUse::HVAC_HEATING),
            "must detect ASHP Heater as HVAC_HEATING equipment"
        );

        // Battery is not HVAC.
        let battery_names = names_of(&schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Battery End Use Electric Power (kW)",
        ]));
        assert!(
            !names_have_equipment_for(&battery_names, &hares_types::EndUse::HVAC_HEATING),
            "must NOT detect Battery as HVAC_HEATING equipment"
        );
        assert!(
            !names_have_equipment_for(&battery_names, &hares_types::EndUse::HVAC_COOLING),
            "must NOT detect Battery as HVAC_COOLING equipment"
        );

        // A surface with no equipment has none.
        let empty = names_of(&schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]));
        assert!(
            !names_have_equipment_for(&empty, &hares_types::EndUse::HVAC_HEATING),
            "an equipment-free surface must not report HVAC equipment"
        );
    }

    /// `names_have_equipment_for` strips instance qualifiers (" #N") from
    /// multi-instance equipment column names.
    #[test]
    fn names_have_equipment_for_strips_instance_qualifiers() {
        let names = names_of(&schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "ASHP Heater #1 Electric Power (kW)",
            "ASHP Heater #2 Electric Power (kW)",
            "HVAC Heating End Use Electric Power (kW)",
        ]));
        assert!(
            names_have_equipment_for(&names, &hares_types::EndUse::HVAC_HEATING),
            "must detect instance-qualified ASHP Heater columns as HVAC_HEATING"
        );
    }

    /// `compute_equipment_counts_from_names` counts per-equipment columns
    /// grouped by EndUse category, skipping aggregate columns.
    #[test]
    #[cfg(feature = "observe")]
    fn compute_equipment_counts_counts_equipment_excluding_aggregates() {
        // Names with 2 HVAC_HEATING equipment, 1 BATTERY equipment, and
        // end-use aggregate columns ("{Display} End Use Electric Power (kW)").
        // "Battery" is both an equipment name and an EndUse display name, so
        // the battery aggregate exercises the case where an aggregate prefix
        // could be mistaken for an equipment instance.
        let names = names_of(&schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "ASHP Heater Electric Power (kW)",
            "Gas Furnace Electric Power (kW)",
            "Battery Electric Power (kW)",
            "HVAC Heating End Use Electric Power (kW)",
            "Battery End Use Electric Power (kW)",
        ]));
        let counts = compute_equipment_counts_from_names(&names);
        assert_eq!(
            counts.get("hvac_heating"),
            Some(&2),
            "must count ASHP Heater + Gas Furnace = 2 HVAC_HEATING equipment"
        );
        assert_eq!(
            counts.get("battery"),
            Some(&1),
            "must count 1 Battery equipment; the Battery aggregate column must not be counted"
        );
        // Aggregate columns (e.g. "HVAC Heating") must not be counted.
        assert!(
            !counts.contains_key("other"),
            "aggregate columns must not be counted as OTHER: got keys {:?}",
            counts.keys().collect::<Vec<_>>()
        );
    }

    /// `MetricsInputPlan::build` populates `end_use_equipment_counts` from
    /// the producer surface so observer diagnostics report non-zero
    /// equipment counts.
    #[test]
    #[cfg(feature = "observe")]
    fn metrics_plan_build_populates_equipment_counts() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "ASHP Heater Electric Power (kW)",
            "HVAC Heating End Use Electric Power (kW)",
        ]);
        let plan = plan_for(&schema, &test_config(None));
        assert!(
            !plan.end_use_equipment_counts.is_empty(),
            "end_use_equipment_counts must be populated from the producer surface"
        );
        assert_eq!(
            plan.end_use_equipment_counts.get("hvac_heating"),
            Some(&1),
            "must count 1 HVAC_HEATING equipment"
        );
    }

    // ── Non-finite value tests ─────────────────────────────────────

    #[test]
    fn value_at_slot_returns_none_for_non_finite() {
        let row = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
        assert_eq!(value_at_slot(&row, 0), None);
        assert_eq!(value_at_slot(&row, 1), None);
        assert_eq!(value_at_slot(&row, 2), None);
    }

    #[test]
    fn value_at_slot_returns_value_for_normal_finite_value() {
        assert_eq!(value_at_slot(&[42.0], 0), Some(42.0));
    }

    #[test]
    fn accumulate_ignores_nan_timestep() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // Row 0: NaN → skipped. Rows 1-2: valid → accumulated.
        accumulate(
            &mut calc,
            &build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![f64::NAN, 1.0, 3.0])]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        assert!(
            !metrics.total_energy_kwh.net_energy_kwh.is_nan(),
            "accumulated total must not be NaN"
        );
        // timestep_h = 1.0, so total = 1.0 + 3.0 = 4.0
        assert!(
            (metrics.total_energy_kwh.net_energy_kwh - 4.0).abs() < 1e-9,
            "total should be 4.0 kWh, got {}",
            metrics.total_energy_kwh.net_energy_kwh
        );
        assert_eq!(
            metrics.nan_step_count, 1,
            "nan_step_count should be 1, got {}",
            metrics.nan_step_count
        );
        assert_eq!(
            metrics.metrics_reliability,
            Reliability::Degraded,
            "metrics_reliability should be Degraded when NaN was detected"
        );
    }

    #[test]
    fn nan_in_power_does_not_poison_envelope() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating Delivered (W)",
            "HVAC Cooling Delivered (W)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // NaN in total electric power; HVAC columns are valid.
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![f64::NAN, 1.0]),
                ("HVAC Heating Delivered (W)", vec![5000.0, 5000.0]),
                ("HVAC Cooling Delivered (W)", vec![-3000.0, -3000.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        let loads = metrics
            .metrics
            .envelope_loads_kwh
            .expect("envelope loads present");
        assert!(
            !loads.hvac_heating_kwh.is_nan(),
            "envelope hvac_heating_kwh must not be NaN"
        );
        assert!(
            !loads.hvac_cooling_kwh.is_nan(),
            "envelope hvac_cooling_kwh must not be NaN"
        );
        // Both rows have valid HVAC data → 2 × 5000 Wh = 10000 Wh = 10 kWh
        assert!(
            (loads.hvac_heating_kwh - 10.0).abs() < 1e-9,
            "hvac heating should be 10.0 kWh, got {}",
            loads.hvac_heating_kwh
        );
        assert!(
            (loads.hvac_cooling_kwh - (-6.0)).abs() < 1e-9,
            "hvac cooling should be -6.0 kWh, got {}",
            loads.hvac_cooling_kwh
        );
    }

    #[test]
    fn nan_power_increments_counter_and_keeps_accumulators_finite() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating End Use Electric Power (kW)",
            "Battery End Use Electric Power (kW)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // Row 0: NaN in total, valid elsewhere
        // Row 1: all valid
        // Row 2: NaN in battery, valid elsewhere
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![f64::NAN, 2.0, 2.0]),
                (
                    "HVAC Heating End Use Electric Power (kW)",
                    vec![1.0, 1.0, 1.0],
                ),
                (
                    "Battery End Use Electric Power (kW)",
                    vec![1.0, 1.0, f64::NAN],
                ),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        // Total: row 0 skipped, rows 1+2 valid = 4 kWh
        assert!(
            metrics.total_energy_kwh.net_energy_kwh.is_finite(),
            "total_energy must be finite"
        );
        assert!(
            (metrics.total_energy_kwh.net_energy_kwh - 4.0).abs() < 1e-9,
            "total should be 4.0 kWh, got {}",
            metrics.total_energy_kwh.net_energy_kwh
        );

        // End-use: hvac_heating: all 3 rows valid = 3 kWh
        let hvac = metrics.total_energy_kwh.per_end_use["hvac_heating"];
        assert!((hvac - 3.0).abs() < 1e-9, "hvac_heating should be 3.0 kWh");

        // Battery: row 2 NaN → skipped, rows 0+1 valid = 2 kWh
        let battery = metrics.total_energy_kwh.per_end_use["battery"];
        assert!((battery - 2.0).abs() < 1e-9, "battery should be 2.0 kWh");

        // counter: 1 NaN in total + 1 NaN in battery = 2
        assert_eq!(metrics.nan_step_count, 2);
    }

    #[test]
    fn inf_values_are_skipped_and_counted() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Cooling End Use Electric Power (kW)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // Three rows: +Inf, valid, -Inf
        accumulate(
            &mut calc,
            &build_batch(vec![
                (
                    TOTAL_ELECTRIC_POWER_KW,
                    vec![f64::INFINITY, 5.0, f64::NEG_INFINITY],
                ),
                (
                    "HVAC Cooling End Use Electric Power (kW)",
                    vec![2.0, 2.0, 2.0],
                ),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        // Total: only row 1 accumulated = 5 kWh
        assert!(
            metrics.total_energy_kwh.net_energy_kwh.is_finite(),
            "total_energy must be finite"
        );
        assert!(
            (metrics.total_energy_kwh.net_energy_kwh - 5.0).abs() < 1e-9,
            "total should be 5.0 kWh, got {}",
            metrics.total_energy_kwh.net_energy_kwh
        );

        // HVAC cooling: all 3 rows valid = 6 kWh
        let cooling = metrics.total_energy_kwh.per_end_use["hvac_cooling"];
        assert!((cooling - 6.0).abs() < 1e-9);

        // 2 Inf values in total column
        assert_eq!(metrics.nan_step_count, 2);
    }

    #[test]
    fn mixed_finite_and_non_finite_sets_reliability_degraded() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating End Use Electric Power (kW)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // Mixed: some finite, some NaN, some Inf
        accumulate(
            &mut calc,
            &build_batch(vec![
                (
                    TOTAL_ELECTRIC_POWER_KW,
                    vec![10.0, f64::NAN, 20.0, f64::INFINITY, 30.0],
                ),
                (
                    "HVAC Heating End Use Electric Power (kW)",
                    vec![5.0, 5.0, f64::NAN, 5.0, 5.0],
                ),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        // Total: 10 + 20 + 30 = 60 kWh (NaN and Inf skipped)
        assert!((metrics.total_energy_kwh.net_energy_kwh - 60.0).abs() < 1e-9);

        // End use: rows 0,1,3,4 valid (row 2 skipped) = 20 kWh
        let hvac = metrics.total_energy_kwh.per_end_use["hvac_heating"];
        assert!((hvac - 20.0).abs() < 1e-9);

        // nan_step_count: 1 NaN + 1 Inf in total + 1 NaN in hvac = 3
        assert_eq!(
            metrics.nan_step_count, 3,
            "expected 3 non-finite skips, got {}",
            metrics.nan_step_count
        );

        // Reliability must be Degraded
        assert_eq!(
            metrics.metrics_reliability,
            Reliability::Degraded,
            "metrics_reliability should be Degraded when non-finite values were skipped"
        );
        assert!(!metrics.metrics_reliability.is_reliable());
    }

    #[test]
    fn all_finite_values_produces_reliable_metrics() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));
        accumulate(
            &mut calc,
            &build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![1.0, 2.0, 3.0])]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        assert_eq!(metrics.nan_step_count, 0);
        assert_eq!(metrics.metrics_reliability, Reliability::Reliable);
        assert!(metrics.metrics_reliability.is_reliable());
    }

    // ── Non-finite-setpoint regression tests ────────────────────────────
    // These tests pin the contract that a missing setpoint value (a
    // non-finite reading in the producer row) skips only the comfort and
    // unmet-hours accumulation, not the envelope, HVAC or battery
    // accumulation.

    #[test]
    fn envelope_loads_accumulate_when_heating_setpoint_is_non_finite() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Window Transmitted Solar Gain (W)",
            "Infiltration Heat Gain - Indoor (W)",
            "Temperature - Indoor (C)",
            "HVAC Heating Setpoint (C)",
            "HVAC Cooling Setpoint (C)",
            "Setpoint Deadband (C)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // Row 0: heating setpoint is non-finite → comfort/unmet skipped,
        //         envelope still accumulates.
        // Row 1: valid setpoints → comfort counted, envelope accumulates.
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0]),
                ("Window Transmitted Solar Gain (W)", vec![1000.0, 500.0]),
                ("Infiltration Heat Gain - Indoor (W)", vec![200.0, 300.0]),
                ("Temperature - Indoor (C)", vec![21.5, 21.5]),
                // Row 0 heating setpoint = NaN
                ("HVAC Heating Setpoint (C)", vec![f64::NAN, 21.0]),
                ("HVAC Cooling Setpoint (C)", vec![23.0, 23.0]),
                ("Setpoint Deadband (C)", vec![1.0, 1.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        let loads = metrics
            .metrics
            .envelope_loads_kwh
            .as_ref()
            .expect("envelope loads present");

        // Row 0: 1000W × 1h = 1.0 kWh; Row 1: 500W × 1h = 0.5 kWh
        assert!(
            (loads.window_solar_kwh - 1.5).abs() < 1e-9,
            "window solar should be 1.5 kWh, got {}",
            loads.window_solar_kwh
        );
        // Row 0: 200W × 1h = 0.2 kWh; Row 1: 300W × 1h = 0.3 kWh
        assert!(
            (loads.infiltration_kwh - 0.5).abs() < 1e-9,
            "infiltration should be 0.5 kWh, got {}",
            loads.infiltration_kwh
        );

        // Only row 1 should contribute to comfort (1 row × 1h)
        assert_eq!(metrics.comfort_hours, Some(1.0));
        assert_eq!(metrics.unmet_load_hours, Some(0.0));

        // 1 of 2 rows lacked valid setpoint data
        let fraction = metrics
            .rows_with_partial_setpoint_data_fraction
            .expect("fraction present when setpoints configured");
        assert!(
            (fraction - 0.5).abs() < 1e-9,
            "fraction should be 0.5, got {}",
            fraction
        );
    }

    #[test]
    fn hvac_thermal_accumulates_when_cooling_setpoint_is_non_finite() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating Delivered (W)",
            "HVAC Cooling Delivered (W)",
            "Temperature - Indoor (C)",
            "HVAC Heating Setpoint (C)",
            "HVAC Cooling Setpoint (C)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(Some(1.0)));

        // Row 0: cooling setpoint is non-finite → comfort/unmet skipped,
        //         HVAC thermal delivered still accumulates.
        // Row 1: valid setpoints → comfort counted, accumulates.
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0]),
                ("HVAC Heating Delivered (W)", vec![4000.0, 3000.0]),
                ("HVAC Cooling Delivered (W)", vec![-2000.0, -1000.0]),
                ("Temperature - Indoor (C)", vec![21.5, 21.5]),
                ("HVAC Heating Setpoint (C)", vec![21.0, 21.0]),
                // Row 0 cooling setpoint = NaN
                ("HVAC Cooling Setpoint (C)", vec![f64::NAN, 23.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        let loads = metrics
            .metrics
            .envelope_loads_kwh
            .as_ref()
            .expect("envelope loads present");

        // Row 0: 4000W × 1h = 4 kWh; Row 1: 3000W × 1h = 3 kWh
        assert!(
            (loads.hvac_heating_kwh - 7.0).abs() < 1e-9,
            "hvac heating should be 7.0 kWh, got {}",
            loads.hvac_heating_kwh
        );
        // Row 0: -2000W × 1h = -2 kWh; Row 1: -1000W × 1h = -1 kWh
        assert!(
            (loads.hvac_cooling_kwh - (-3.0)).abs() < 1e-9,
            "hvac cooling should be -3.0 kWh, got {}",
            loads.hvac_cooling_kwh
        );

        // Only row 1 should contribute to comfort
        assert_eq!(metrics.comfort_hours, Some(1.0));
        assert_eq!(metrics.unmet_load_hours, Some(0.0));

        let fraction = metrics
            .rows_with_partial_setpoint_data_fraction
            .expect("fraction present");
        assert!((fraction - 0.5).abs() < 1e-9);
    }

    #[test]
    fn battery_energy_accumulates_when_setpoints_are_non_finite() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Battery End Use Electric Power (kW)",
            "Temperature - Indoor (C)",
            "HVAC Heating Setpoint (C)",
            "HVAC Cooling Setpoint (C)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(Some(1.0)));

        // Row 0: both setpoints are non-finite → comfort/unmet skipped,
        //         battery energy still accumulated.
        // Row 1: valid setpoints → comfort counted, battery accumulates.
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![10.0, 5.0]),
                ("Battery End Use Electric Power (kW)", vec![10.0, -5.0]),
                ("Temperature - Indoor (C)", vec![21.5, 21.5]),
                // Both setpoints NaN for row 0
                ("HVAC Heating Setpoint (C)", vec![f64::NAN, 21.0]),
                ("HVAC Cooling Setpoint (C)", vec![f64::NAN, 23.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        // Row 0: 10 kW × 1h = 10 kWh in
        // Row 1: -5 kW × 1h = 5 kWh out
        let rte = metrics
            .efficiency
            .battery_round_trip_efficiency
            .expect("RTE should be present");
        assert!(
            (rte - 0.5).abs() < 1e-9,
            "RTE should be 5/10 = 0.5, got {}",
            rte
        );

        // Only row 1 comfort
        assert_eq!(metrics.comfort_hours, Some(1.0));
        assert_eq!(metrics.unmet_load_hours, Some(0.0));

        let fraction = metrics
            .rows_with_partial_setpoint_data_fraction
            .expect("fraction present");
        assert!((fraction - 0.5).abs() < 1e-9);
    }

    #[test]
    fn deadband_invalid_does_not_skip_envelope() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "Window Transmitted Solar Gain (W)",
            "HVAC Heating Delivered (W)",
            "Temperature - Indoor (C)",
            "HVAC Heating Setpoint (C)",
            "HVAC Cooling Setpoint (C)",
            "Setpoint Deadband (C)",
        ]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // Row 0: deadband is negative → invalid, comfort/unmet skipped,
        //         envelope and HVAC thermal still accumulate.
        // Row 1: deadband = 0.0 → valid (zero deadband), comfort counted.
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0]),
                ("Window Transmitted Solar Gain (W)", vec![800.0, 400.0]),
                ("HVAC Heating Delivered (W)", vec![2000.0, 1000.0]),
                ("Temperature - Indoor (C)", vec![21.5, 21.5]),
                ("HVAC Heating Setpoint (C)", vec![21.0, 21.0]),
                ("HVAC Cooling Setpoint (C)", vec![23.0, 23.0]),
                // Row 0: negative deadband (invalid); Row 1: zero (valid)
                ("Setpoint Deadband (C)", vec![-1.0, 0.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        let loads = metrics
            .metrics
            .envelope_loads_kwh
            .as_ref()
            .expect("envelope loads present");

        // Row 0: 800W × 1h = 0.8 kWh; Row 1: 400W × 1h = 0.4 kWh
        assert!(
            (loads.window_solar_kwh - 1.2).abs() < 1e-9,
            "window solar should be 1.2 kWh, got {}",
            loads.window_solar_kwh
        );
        // Row 0: 2000W × 1h = 2 kWh; Row 1: 1000W × 1h = 1 kWh
        assert!(
            (loads.hvac_heating_kwh - 3.0).abs() < 1e-9,
            "hvac heating should be 3.0 kWh, got {}",
            loads.hvac_heating_kwh
        );

        // Only row 1 comfort (deadband is zero, temp inside)
        assert_eq!(metrics.comfort_hours, Some(1.0));

        // 1 of 2 rows had invalid deadband
        let fraction = metrics
            .rows_with_partial_setpoint_data_fraction
            .expect("fraction present");
        assert!((fraction - 0.5).abs() < 1e-9);
    }

    // ── Duration coverage and leap-year normalization tests ────────────

    #[test]
    fn leap_year_energy_is_normalized_to_standard_year() {
        // 2024 is a leap year (366 d = 8784 h). With 1 kW constant load,
        // raw accumulated = 8784 kWh; normalized should be 8760 kWh.
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating End Use Electric Power (kW)",
        ]);
        let config = test_config_full_year(2024, None);
        let mut calc = calc_for(&schema, 3600, &config);

        let rows = 8_784;
        let batch = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0; rows]),
            ("HVAC Heating End Use Electric Power (kW)", vec![1.0; rows]),
        ]);
        accumulate(&mut calc, &batch).unwrap();
        let metrics = calc.finish().unwrap();

        // Normalized total should be 8760 kWh (not 8784).
        assert!(
            (metrics.total_energy_kwh.net_energy_kwh - 8_760.0).abs() < 1e-9,
            "leap-year total should be normalized to 8760 kWh, got {}",
            metrics.total_energy_kwh.net_energy_kwh
        );
        assert!(
            (metrics.total_energy_kwh.duration_hours - 8_784.0).abs() < 1e-9,
            "duration_hours should reflect actual 8784 h"
        );
        assert_eq!(metrics.coverage, SimulationCoverage::LeapYear);
        // Per-end-use should also be normalized.
        let hvac = metrics.total_energy_kwh.per_end_use["hvac_heating"];
        assert!(
            (hvac - 8_760.0).abs() < 1e-9,
            "per-end-use leap-year energy should be normalized to 8760 kWh, got {}",
            hvac
        );
    }

    #[test]
    fn partial_year_is_reported_as_partial_coverage() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let config = test_config_full_year(2026, None);
        let mut calc = calc_for(&schema, 3600, &config);

        // 4380 hours = half a year.
        let rows = 4_380;
        let batch = build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![1.0; rows])]);
        accumulate(&mut calc, &batch).unwrap();
        let metrics = calc.finish().unwrap();

        assert_eq!(metrics.coverage, SimulationCoverage::PartialYear);
        assert!((metrics.total_energy_kwh.net_energy_kwh - 4_380.0).abs() < 1e-9);
        assert!((metrics.simulation_duration_hours - 4_380.0).abs() < 1e-9);
    }

    #[test]
    fn multi_year_is_reported_as_multi_coverage() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let config = test_config_full_year(2026, None);
        let mut calc = calc_for(&schema, 3600, &config);

        // 17520 hours = 2 years.
        let rows = 17_520;
        let batch = build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![1.0; rows])]);
        accumulate(&mut calc, &batch).unwrap();
        let metrics = calc.finish().unwrap();

        assert_eq!(metrics.coverage, SimulationCoverage::MultiYear);
        // Multi-year data is not normalized.
        assert!((metrics.total_energy_kwh.net_energy_kwh - 17_520.0).abs() < 1e-9);
        assert!((metrics.simulation_duration_hours - 17_520.0).abs() < 1e-9);
    }

    #[test]
    fn leap_year_gas_energy_is_normalized_to_standard_year() {
        // Gas energy must be normalized alongside electric in leap-year
        // simulations so that combined_total_energy_kwh() is on a
        // consistent time base.
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, TOTAL_GAS_POWER_THERMS]);
        let config = test_config_full_year(2024, None); // 2024 = leap year
        let mut calc = calc_for(&schema, 3600, &config);

        let rows = 8_784;
        let batch = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.0; rows]),
            // 1 therm/h for 8784 h → raw = 8784 therms;
            // normalized (× 8760/8784) → 8760 therms.
            (TOTAL_GAS_POWER_THERMS, vec![1.0; rows]),
        ]);
        accumulate(&mut calc, &batch).unwrap();
        let metrics = calc.finish().unwrap();

        assert_eq!(metrics.coverage, SimulationCoverage::LeapYear);

        let gas = metrics
            .gas_energy
            .as_ref()
            .expect("gas energy should be present");
        // Normalized gas therms: 8784 × (8760/8784) = 8760 therms.
        assert!(
            (gas.total_therms - 8_760.0).abs() < 1e-6,
            "leap-year gas therms should be normalized to 8760, got {}",
            gas.total_therms
        );
        assert!(
            (gas.total_kwh_equivalent - energy_therms_to_kwh(8_760.0)).abs() < 1e-3,
            "leap-year gas kwh equivalent should be normalized to 8760 therms"
        );

        // Combined total: 8760 (electric) + energy_therms_to_kwh(8760) (gas)
        let expected_combined = 8_760.0 + energy_therms_to_kwh(8_760.0);
        assert!(
            (metrics.combined_total_energy_kwh() - expected_combined).abs() < 1e-3,
            "combined_total_energy_kwh should sum normalized electric and gas"
        );
    }

    // ── Net vs gross energy tests ────────────────────────────────────────

    #[test]
    fn finish_separates_net_consumption_and_pv_generation() {
        let schema =
            schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "PV End Use Electric Power (kW)"]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // Mixed-sign power: 3 kW import, 2 kW import, -1 kW export (PV generation)
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![3.0, 2.0, -1.0]),
                ("PV End Use Electric Power (kW)", vec![0.0, 0.0, -1.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        // net = 3 + 2 + (-1) = 4 kWh
        assert!(
            (metrics.total_energy_kwh.net_energy_kwh - 4.0).abs() < 1e-9,
            "net_energy_kwh should be 4 kWh, got {}",
            metrics.total_energy_kwh.net_energy_kwh
        );
        // gross_consumption = 3 + 2 = 5 kWh (only positive rows)
        assert!(
            (metrics.total_energy_kwh.gross_consumption_kwh - 5.0).abs() < 1e-9,
            "gross_consumption_kwh should be 5 kWh, got {}",
            metrics.total_energy_kwh.gross_consumption_kwh
        );
        // gross_pv = |-1| = 1 kWh (abs of negative PV rows)
        assert!(
            (metrics.total_energy_kwh.gross_pv_generation_kwh - 1.0).abs() < 1e-9,
            "gross_pv_generation_kwh should be 1 kWh, got {}",
            metrics.total_energy_kwh.gross_pv_generation_kwh
        );
    }

    #[test]
    fn zero_net_with_high_gross_consumption_and_pv_is_correct() {
        // Regression: a PV dwelling with balanced import/export
        // must report gross_consumption > 0, gross_pv > 0, net ≈ 0
        let schema =
            schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "PV End Use Electric Power (kW)"]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        // 5 kWh imported, 5 kWh exported via PV → net ≈ 0
        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![5.0, -5.0]),
                ("PV End Use Electric Power (kW)", vec![0.0, -5.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        // net ≈ 0 (balanced)
        assert!(
            metrics.total_energy_kwh.net_energy_kwh.abs() < 1e-9,
            "net_energy_kwh should be ~0 for balanced PV dwelling, got {}",
            metrics.total_energy_kwh.net_energy_kwh
        );
        // gross_consumption = 5 kWh
        assert!(
            (metrics.total_energy_kwh.gross_consumption_kwh - 5.0).abs() < 1e-9,
            "gross_consumption_kwh should be 5 kWh even when net ≈ 0, got {}",
            metrics.total_energy_kwh.gross_consumption_kwh
        );
        // gross_pv = 5 kWh
        assert!(
            (metrics.total_energy_kwh.gross_pv_generation_kwh - 5.0).abs() < 1e-9,
            "gross_pv_generation_kwh should be 5 kWh even when net ≈ 0, got {}",
            metrics.total_energy_kwh.gross_pv_generation_kwh
        );
    }

    #[test]
    fn all_negative_power_gives_negative_net_positive_gross_pv() {
        // All PV generation, no import → net is negative, gross_consumption = 0
        let schema =
            schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, "PV End Use Electric Power (kW)"]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![-1.0, -2.0]),
                ("PV End Use Electric Power (kW)", vec![-1.0, -2.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        assert!(
            metrics.total_energy_kwh.net_energy_kwh < 0.0,
            "net_energy_kwh should be negative for pure generation"
        );
        // gross_consumption = 0 (no positive rows)
        assert!(
            metrics.total_energy_kwh.gross_consumption_kwh == 0.0,
            "gross_consumption_kwh should be 0 for pure generation, got {}",
            metrics.total_energy_kwh.gross_consumption_kwh
        );
        // gross_pv = |-1| + |-2| = 3 kWh
        assert!(
            (metrics.total_energy_kwh.gross_pv_generation_kwh - 3.0).abs() < 1e-9,
            "gross_pv_generation_kwh should be 3 kWh, got {}",
            metrics.total_energy_kwh.gross_pv_generation_kwh
        );
    }

    #[test]
    fn combined_total_energy_kwh_includes_gas_when_present() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW, TOTAL_GAS_POWER_THERMS]);
        let mut calc = calc_for(&schema, 3600, &test_config(None));

        accumulate(
            &mut calc,
            &build_batch(vec![
                (TOTAL_ELECTRIC_POWER_KW, vec![10.0]),
                (TOTAL_GAS_POWER_THERMS, vec![5.0]),
            ]),
        )
        .unwrap();
        let metrics = calc.finish().unwrap();

        let expected = 10.0 + energy_therms_to_kwh(5.0);
        assert!(
            (metrics.combined_total_energy_kwh() - expected).abs() < 1e-6,
            "combined should be {expected}, got {}",
            metrics.combined_total_energy_kwh()
        );
    }

    /// A metrics accumulator snapshot applied to a fresh calculator
    /// continues the run bitwise: the fresh calculator feeds the same
    /// post-snapshot rows as the original and both finish to metrics every
    /// float of which carries the same bits.
    #[test]
    fn metrics_snapshot_restores_a_continuing_calculator_bitwise() {
        let schema = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            TOTAL_GAS_POWER_THERMS,
            "HVAC Heating End Use Electric Power (kW)",
            "Temperature - Indoor (C)",
        ]);
        let config = test_config(None);
        let mut continuous = calc_for(&schema, 900, &config);
        let mut interrupted = calc_for(&schema, 900, &config);

        let pre = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![1.5, 2.5, 3.5, 0.25]),
            (TOTAL_GAS_POWER_THERMS, vec![0.5, 0.5, 0.75, 0.0]),
            (
                "HVAC Heating End Use Electric Power (kW)",
                vec![0.8, 1.2, 0.0, 0.0],
            ),
            ("Temperature - Indoor (C)", vec![21.0, 22.5, 19.0, 23.0]),
        ]);
        accumulate(&mut continuous, &pre).expect("feed");
        accumulate(&mut interrupted, &pre).expect("feed");

        // The checkpoint: the snapshot crosses JSON, the checkpoint's own
        // encoding.
        let snapshot = continuous.snapshot();
        let json = serde_json::to_string(&snapshot).expect("serialize the snapshot");
        let decoded: MetricsSnapshot = serde_json::from_str(&json).expect("decode the snapshot");
        interrupted
            .apply_snapshot(&decoded)
            .expect("the snapshot applies");

        let post = build_batch(vec![
            (TOTAL_ELECTRIC_POWER_KW, vec![4.5, 5.5, 0.5, 2.05]),
            (TOTAL_GAS_POWER_THERMS, vec![1.25, 0.0, 0.0, 0.5]),
            (
                "HVAC Heating End Use Electric Power (kW)",
                vec![1.4, 1.6, 0.0, 0.2],
            ),
            ("Temperature - Indoor (C)", vec![20.0, 21.5, 22.0, 20.5]),
        ]);
        accumulate(&mut continuous, &post).expect("feed");
        accumulate(&mut interrupted, &post).expect("feed");

        let a = continuous.finish().expect("finish");
        let b = interrupted.finish().expect("finish");
        let bits = |m: &FullSimulationMetrics| {
            serde_json::to_string(m).expect("the metrics encode (equal bits, equal bytes)")
        };
        assert_eq!(
            bits(&a),
            bits(&b),
            "the resumed accumulator's metrics are bitwise the continuous run's"
        );
    }

    /// A snapshot whose input plan was built against a different producer
    /// surface is refused, naming the section.
    #[test]
    fn metrics_snapshot_rejects_a_foreign_plan() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let config = test_config(None);
        let mut calc = calc_for(&schema, 900, &config);
        accumulate(
            &mut calc,
            &build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![2.0])]),
        )
        .unwrap();
        let snapshot = calc.snapshot();

        let wide = schema_from_columns(&[
            TOTAL_ELECTRIC_POWER_KW,
            "HVAC Heating End Use Electric Power (kW)",
        ]);
        let mut other = calc_for(&wide, 900, &config);
        let err = other
            .apply_snapshot(&snapshot)
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("metrics snapshot") && err.contains("input plan"),
            "the refusal names the metrics section and its plan: {err}"
        );
    }

    /// The rolling peak's window state (the ring buffer's position and
    /// contents, the running peaks) restores, so a resumed run's 15/30/60
    /// minute peaks are the continuous run's.
    #[test]
    fn metrics_snapshot_carries_the_rolling_peak_window() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let config = test_config(None);
        // 900 s steps: the 15-minute window holds 4 readings.
        let mut continuous = calc_for(&schema, 900, &config);
        let mut resumed = calc_for(&schema, 900, &config);

        let pre = build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![3.0, 4.0, 5.0])]);
        accumulate(&mut continuous, &pre).unwrap();
        accumulate(&mut resumed, &pre).unwrap();

        resumed
            .apply_snapshot(&continuous.snapshot())
            .expect("the snapshot applies");

        let post = build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![1.0, 1.0, 2.0])]);
        accumulate(&mut continuous, &post).unwrap();
        accumulate(&mut resumed, &post).unwrap();

        let a = continuous.finish().unwrap().metrics.peak_power_kw;
        let b = resumed.finish().unwrap().metrics.peak_power_kw;
        assert_eq!(
            a.rolling.peak_15min_kw.to_bits(),
            b.rolling.peak_15min_kw.to_bits(),
            "the 15-minute rolling peak is bitwise the continuous run's"
        );
        assert_eq!(
            a.rolling.peak_30min_kw.to_bits(),
            b.rolling.peak_30min_kw.to_bits()
        );
        assert_eq!(
            a.rolling.peak_60min_kw.to_bits(),
            b.rolling.peak_60min_kw.to_bits()
        );
    }

    /// A snapshot whose ring does not fit the receiver's resolution is
    /// refused by the typed error naming both lengths, in debug and
    /// release alike (a silent skip would leave the resumed run's rolling
    /// peaks zeroed). The resolutions here agree: the ring check is the
    /// restore's own backstop behind the resolution gate.
    #[test]
    fn metrics_snapshot_rejects_a_ring_that_does_not_fit() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let config = test_config(None);
        let mut source = calc_for(&schema, 900, &config);
        accumulate(
            &mut source,
            &build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![5.0])]),
        )
        .unwrap();
        let mut snapshot = source.snapshot();
        // A ring one reading longer than the 900 s resolution's 4: the
        // resolution gate passes it, the ring's own check refuses.
        snapshot.accumulators.rolling_peak.buffer.push(0.0);

        let mut target = calc_for(&schema, 900, &config);
        let err = target
            .apply_snapshot(&snapshot)
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("rolling peak ring")
                && err.contains("holds 5 readings")
                && err.contains("needs 4"),
            "the refusal is the typed snapshot error naming both ring \
             lengths: {err}"
        );
    }

    /// A snapshot written at another time resolution is refused, naming
    /// both resolutions: the input plan is resolution-free, so the
    /// refusal is the carried resolution itself.
    #[test]
    fn metrics_snapshot_rejects_a_foreign_time_resolution_by_name() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let config = test_config(None);
        let mut source = calc_for(&schema, 900, &config);
        accumulate(
            &mut source,
            &build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![5.0])]),
        )
        .unwrap();
        let snapshot = source.snapshot();

        let mut target = calc_for(&schema, 300, &config);
        let err = target
            .apply_snapshot(&snapshot)
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("time resolution 900 s") && err.contains("is 300 s"),
            "the refusal names both resolutions: {err}"
        );
    }

    /// Two resolutions whose rings collide (900 s and 1000 s both fill a
    /// 4-reading ring) are still distinct: the snapshot carries the time
    /// resolution and the restore refuses the other one, ring equality
    /// notwithstanding (a blended ring would mix readings taken at
    /// different intervals into the same windows).
    #[test]
    fn metrics_snapshot_rejects_a_resolution_its_ring_collides_with() {
        let schema = schema_from_columns(&[TOTAL_ELECTRIC_POWER_KW]);
        let config = test_config(None);
        let mut source = calc_for(&schema, 900, &config);
        accumulate(
            &mut source,
            &build_batch(vec![(TOTAL_ELECTRIC_POWER_KW, vec![5.0])]),
        )
        .unwrap();
        let snapshot = source.snapshot();

        let mut target = calc_for(&schema, 1000, &config);
        let err = target
            .apply_snapshot(&snapshot)
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("time resolution 900 s") && err.contains("is 1000 s"),
            "the refusal names both resolutions: {err}"
        );
    }
}
