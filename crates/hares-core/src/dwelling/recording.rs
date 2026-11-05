//! The dwelling's recording layer: the frozen producer bindings and the
//! step's record path.
//!
//! The schema-facing half of the recording (the bindings, the zone column
//! caches and their builders) lives in `hares-output::recording`; this
//! module holds the layer type the dwelling owns: the bindings beside
//! their producer half (the envelope gain columns, whose extractors are
//! typed on the envelope solver's component gains), the report
//! projections, and the step's record path, which writes the row by
//! precomputed index and feeds every report's recorder.
//!
//! The freeze contract: every binding, envelope column and projection
//! resolves once, at the row plan's freeze, against one column index of
//! one schema; a roster change that keeps the frozen schema re-resolves
//! only the roster-facing bindings. The steady-state step allocates
//! nothing here: the row and the projected-row buffers are the layer's
//! own, reused every step, and every read is by precomputed index.
//!
//! The producers' values enter as borrowed views ([`StepRecordInputs`]);
//! the layer reads no dwelling state beyond them, so the record path is
//! the same writes in the same order whether the dwelling steps once or a
//! fleet steps many.

use std::collections::HashMap;

use hares_envelope::EnvelopeComponentGains;
use hares_equipment::Equipment;
use hares_output::recording::{DiagnosticsBindings, OutputRowBindings, ZoneColumnCaches};
use hares_output::{MetricsCalculator, OutputError, RowWriter, StreamingRecorder};
use hares_types::{EnvironmentState, HaresError, PortSlots, ZoneId, telemetry_keys as tk};

use crate::Actor;

use super::{Result, StepResult};

/// One frozen envelope column: the gain term that produces its value and
/// the slot it writes.
pub(super) type EnvelopeColumn = (fn(&EnvelopeComponentGains) -> f64, Option<usize>);

/// The gain terms the envelope columns read, as (column name, extractor)
/// pairs in a fixed order. Frozen once into the recording layer's
/// envelope columns with each name's resolved slot.
type EnvelopeTerm = (&'static str, fn(&EnvelopeComponentGains) -> f64);

#[cfg_attr(not(feature = "observe"), allow(unused_mut))]
fn envelope_column_terms() -> Vec<EnvelopeTerm> {
    let mut terms: Vec<EnvelopeTerm> = vec![
        // Transmitted (through-glass) flux only, the OCHRE column
        // definition (Envelope.py:1160); the inward-flowing absorbed
        // share is part of `window_solar_w`, not this term.
        ("Window Transmitted Solar Gain (W)", |g| {
            g.window_through_glass_w
        }),
        ("Infiltration Heat Gain - Indoor (W)", |g| g.infiltration_w),
        ("Forced Ventilation Heat Gain - Indoor (W)", |g| {
            g.ventilation_w
        }),
        ("Natural Ventilation Heat Gain - Indoor (W)", |g| {
            g.natural_ventilation_w
        }),
        // Net sensible = direct heat injections on zone air, matching
        // OCHRE's "Net Sensible Heat Gain - {zone} (W)" (zone heat input:
        // occupancy, HVAC, equipment, infiltration, ventilation, window
        // solar). Opaque envelope conduction reaches the zone through the
        // A-matrix surface coupling rather than a direct injection, so it
        // is reported per boundary below instead of here. The gross
        // exterior solar + LWR absorption (`opaque_solar_lwr_w`) is a
        // boundary condition on the exterior RC nodes, not a zone input,
        // and is excluded entirely.
        ("Net Sensible Heat Gain - Indoor (W)", |g| {
            g.window_solar_w
                + g.infiltration_w
                + g.ventilation_w
                + g.natural_ventilation_w
                + g.internal_gain_w
                + g.jacket_loss_w
                + g.duct_loss_w
                + g.hvac_heating_w.max(0.0)
                + g.hvac_cooling_w
        }),
        ("Internal Heat Gain - Indoor (W)", |g| g.internal_gain_w),
        ("Interior LWR Exchange - Indoor (W)", |g| g.interior_lwr_w),
        // Net conduction from the opaque envelope into the zone (wall +
        // floor + roof interior surfaces) — the inside-face convection
        // term of the zone air heat balance (EnergyPlus ERM 26.1 —
        // "Inside Heat Balance": Interior Convection), aggregated from
        // the three per-boundary terms below. The gross exterior solar +
        // LWR absorption is an outside-face balance driver
        // (EnergyPlus ERM 26.1 — "Outside Surface Heat Balance"): most of
        // it re-leaves via exterior convection and sky exchange, so it is
        // not a load on the conditioned zone.
        ("Opaque Surface Heat Gain - Indoor (W)", |g| {
            g.wall_heat_gain_w + g.floor_heat_gain_w + g.roof_heat_gain_w
        }),
        ("Duct Loss Heat Gain - Indoor (W)", |g| g.duct_loss_w),
        ("Roof Heat Gain - Indoor (W)", |g| g.roof_heat_gain_w),
        ("Floor Heat Gain - Indoor (W)", |g| g.floor_heat_gain_w),
        ("Wall Heat Gain - Indoor (W)", |g| g.wall_heat_gain_w),
        ("Window Heat Gain - Indoor (W)", |g| g.window_heat_gain_w),
        ("Internal Mass Heat Gain - Indoor (W)", |g| {
            g.internal_mass_heat_gain_w
        }),
        ("HVAC Heating Delivered (W)", |g| g.hvac_heating_w),
        ("HVAC Cooling Delivered (W)", |g| g.hvac_cooling_w.abs()),
        // Complete zone air heat-balance residual (E+ Output:Diagnostics
        // analogue): C_zone·ΔT/dt − Σ(all gain terms). Small in a correct
        // model; O(kW) values mean a mis-wired or mislabeled gain — the
        // I-02 defect class made visible in every output file.
        ("Zone Air Heat Balance Residual (W)", |g| {
            g.zone_air_balance_residual_w
        }),
    ];
    #[cfg(feature = "observe")]
    terms.extend([
        (
            "Natural Ventilation Cw (-)",
            (|g: &EnvelopeComponentGains| g.natural_ventilation_cw)
                as fn(&EnvelopeComponentGains) -> f64,
        ),
        (
            "Natural Ventilation Wind Angle (deg)",
            (|g: &EnvelopeComponentGains| g.natural_ventilation_wind_angle_deg)
                as fn(&EnvelopeComponentGains) -> f64,
        ),
    ]);
    terms
}

/// Resolves the envelope gain columns of the frozen row plan against the
/// freeze-time column index: the gain terms' names to their producer
/// slots. Runs once per plan build, never per step; the step path calls
/// the frozen extractors and never looks a name up.
pub(super) fn build_envelope_columns(column_index: &HashMap<String, usize>) -> Vec<EnvelopeColumn> {
    envelope_column_terms()
        .into_iter()
        .map(|(name, term)| (term, column_index.get(name).copied()))
        .collect()
}

/// The dwelling's recording layer: the frozen producer bindings and the
/// step's record path's own state. Installed by every roster change's
/// plan (the bindings follow the roster, the rest the schema freeze).
#[derive(Default)]
pub(super) struct RecordingLayer {
    /// The row plan's producer bindings, resolved at freeze: the
    /// dwelling-level columns, the per-equipment columns, the end-use
    /// aggregates and the actor columns.
    pub(super) bindings: OutputRowBindings,
    /// The envelope gain columns, frozen beside the bindings against the
    /// same column index: the gain term that feeds each slot.
    pub(super) envelope_columns: Vec<EnvelopeColumn>,
    /// The pre-resolved zone column caches: the record path's writes
    /// (temperature, infiltration, interior LWR, per-zone HVAC
    /// attribution) go through them, and the state-snapshot phase's
    /// zone-temperature fill reads the environment indices.
    pub(super) zones: ZoneColumnCaches,
    /// The row plan's per-report projections, copied at freeze: the layer
    /// pushes each report's projected row to its recorder through these
    /// indices.
    pub(super) projections: Vec<Vec<usize>>,
    /// Reused scratch for the per-report projected rows: the union row's
    /// values one report's projection selects, pushed to that report's
    /// recorder. Cleared and refilled per report per step.
    projected_row_buf: Vec<f64>,
    /// The projected row's validity, parallel to `projected_row_buf`, fed
    /// to the recorders with every pushed row.
    projected_valid_buf: Vec<bool>,
}

/// The step's producer values, borrowed for one record call: everything
/// the record path reads of the dwelling, nothing more. Every field is a
/// borrow or a copy, so the steady-state record allocates nothing.
pub(super) struct StepRecordInputs<'a> {
    /// The step's own observable output (the timestamp, the net electric
    /// power, the HVAC delivery and gas totals the dwelling-level columns
    /// read).
    pub(super) step: &'a StepResult,
    /// The step's environment state: the weather channels and the current
    /// time the environment columns read.
    pub(super) env: &'a EnvironmentState,
    /// The port slots, still holding the step's equipment contributions
    /// (`ports.zero()` runs after the record): the fuel and thermal
    /// attribution columns read them.
    pub(super) ports: &'a PortSlots,
    /// The step's number, the diagnostics row's `step` column.
    pub(super) step_number: u64,
    /// The solver's net reactive power, the `Total Reactive Power (kVAR)`
    /// column's value.
    pub(super) net_reactive_var: f64,
    /// The step's zone temperatures, in the roster's zone order (the
    /// reused scratch the state-snapshot phase filled).
    pub(super) zone_temps: &'a [(ZoneId, f64)],
    /// Per equipment, index-aligned with `equipment`: whether its step
    /// failed and was rolled back (the flow columns record no flow).
    pub(super) step_failed: &'a [bool],
    /// The equipment roster, in vector order: the core outputs and the
    /// telemetry the per-equipment columns read.
    pub(super) equipment: &'a [Box<dyn Equipment>],
    /// The actor roster, in registration order: the telemetry the actor
    /// columns read.
    pub(super) actors: &'a [Box<dyn Actor>],
    /// The envelope solver's component gains for the step: the envelope
    /// gain columns' and the diagnostics block's producer values.
    pub(super) gains: &'a EnvelopeComponentGains,
}

/// The actor telemetry views the bindings' build reads: each actor's
/// name and telemetry, in actor order. The `Actor` trait is the
/// dwelling's; the bindings' builder reads exactly this slice of it.
pub(super) fn actor_telemetry_views<'a>(
    actors: &[&'a dyn Actor],
) -> Vec<(&'a str, Option<&'a hares_types::Telemetry>)> {
    actors.iter().map(|a| (a.name(), a.telemetry())).collect()
}

impl RecordingLayer {
    /// Writes one step's row and feeds every report's recorder: the
    /// dwelling-level columns, the per-equipment columns, the end-use
    /// aggregates, the actor telemetry columns, the zone columns, the
    /// envelope gain columns and the diagnostics block, then the run
    /// metrics' accumulation and the per-report projected pushes.
    ///
    /// Every write is by precomputed index; the write order is the
    /// producer order the row plan froze. A callback error ends the run
    /// the way the failure budget does (the caller latches it); every
    /// other recorder failure is an I/O error.
    pub(super) fn record_step(
        &mut self,
        writer: &mut RowWriter<'_>,
        inputs: &StepRecordInputs<'_>,
        metrics: Option<&mut MetricsCalculator>,
        recorders: &mut [StreamingRecorder],
    ) -> Result<()> {
        // The row's Time: the step's naive local wall clock, written once
        // per row as the milliseconds the Timestamp(Millisecond, None)
        // field carries.
        let time_ms = inputs.step.timestamp.and_utc().timestamp_millis();
        let dwelling_columns = &self.bindings.dwelling;

        if let Some(idx) = dwelling_columns.total_electric {
            writer.write(idx, inputs.step.net_electric_power_w);
        }
        if let Some(idx) = dwelling_columns.total_gas {
            // The value is W (SI); the column's declared therms/hour unit
            // converts once in the writer.
            let gas_w = inputs.ports.fuel.get(hares_types::FuelType::Gas);
            writer.write(idx, gas_w);
        }
        if let Some(idx) = dwelling_columns.total_reactive {
            writer.write(idx, inputs.net_reactive_var);
        }

        // Per-equipment columns via the frozen plan's producer bindings. The
        // state columns (mode, setpoint, SOC, schedule) read the committed
        // state; the flow and telemetry columns read what the equipment
        // delivered this step, which for a failed-and-rolled-back step is
        // nothing.
        for (eq_idx, (eq, cols)) in inputs
            .equipment
            .iter()
            .zip(&self.bindings.equipment)
            .enumerate()
        {
            let co = eq.core_output();
            if let Some(idx) = cols.mode {
                writer.write_opt(idx, co.state.operating_mode.map(|m| m.as_code()));
            }
            if let Some(idx) = cols.setpoint {
                writer.write_opt(idx, co.state.setpoint_c);
            }
            if let Some(idx) = cols.soc {
                writer.write_opt(idx, co.state.soc.map(|soc| soc.get()));
            }
            if let Some(idx) = cols.schedule {
                let is_active = co
                    .state
                    .operating_mode
                    .is_some_and(|m| m != hares_types::OperatingMode::Off);
                writer.write(idx, if is_active { 1.0 } else { 0.0 });
            }
            if let Some(idx) = cols.speed {
                writer.write_opt(idx, co.state.speed_index.map(f64::from));
            }
            if let Some(idx) = cols.defrost_state {
                writer.write_opt(idx, eq.telemetry().get(tk::DEFROST_CYCLE_STATE)); // allowed: defrost cycle state remains telemetry-only until CoreOutput gains a defrost_state field.
            }
            for &(key, idx) in &cols.v8_state_columns {
                writer.write_opt(idx, eq.telemetry().get(key)); // allowed: v8 telemetry pass-through uses pre-resolved tk:: constants bound to `key`.
            }
            if inputs.step_failed[eq_idx] {
                // The step failed and was rolled back: the equipment
                // delivered nothing this step. The not-nullable flow
                // columns read their inactive zeros (the same zeros the
                // success path's substitutions give an inactive
                // component), and the nullable ones stay unwritten, null.
                if let Some(idx) = cols.electric_power {
                    writer.write(idx, 0.0);
                }
                if let Some(idx) = cols.gas_power {
                    writer.write(idx, 0.0);
                }
                if let Some(idx) = cols.capacity {
                    writer.write(idx, 0.0);
                }
                if let Some(idx) = cols.reactive_power {
                    writer.write(idx, 0.0);
                }
                if let Some(idx) = cols.energy_kwh {
                    writer.write(idx, 0.0);
                }
                if let Some(idx) = cols.er_power {
                    writer.write(idx, 0.0);
                }
                if let Some(idx) = cols.fan_power {
                    writer.write(idx, 0.0);
                }
                if let Some(idx) = cols.main_power {
                    writer.write(idx, 0.0);
                }
                if let Some(idx) = cols.runtime_fraction {
                    writer.write(idx, 0.0);
                }
                if let Some(idx) = cols.latent_gains {
                    writer.write(idx, 0.0);
                }
                continue;
            }
            if let Some(idx) = cols.electric_power {
                // Not nullable: the quantity is physically zero when its
                // component is inactive (an undeclared channel is the zero,
                // the declared one's absence the contract's wiring defect).
                writer.write(
                    idx,
                    co.flows.electric_w.map_or(0.0, |e| e.net_consumption_w()),
                );
            }
            if let Some(idx) = cols.gas_power {
                writer.write(idx, co.flows.fuel_w.map_or(0.0, |f| f.consumption_w));
            }
            if let Some(idx) = cols.capacity {
                // Capacity column is always a positive magnitude (OCHRE HVAC.py:598:
                // `capacity_list` is asserted strictly positive). thermal_output_w uses
                // the signed convention (negative for cooling), so take abs().
                // Not nullable: 0.0 when the equipment reports no thermal
                // channel (inactive).
                let capacity_w = co
                    .flows
                    .thermal_output_w
                    .map(f64::abs)
                    .or(co.flows.sensible_cooling_w.map(f64::abs))
                    .unwrap_or(0.0);
                writer.write(idx, capacity_w);
            }
            if let Some(idx) = cols.cop {
                // Nullable: the COP of a step with no compressor input is
                // undefined, the cell null (today a 0.0 sentinel).
                writer.write_opt(idx, co.performance.cop);
            }
            if let Some(idx) = cols.reactive_power {
                let q = co.flows.reactive_power_var.unwrap_or(0.0);
                writer.write(idx, q);
                if let Some(pf_idx) = cols.power_factor {
                    let p = co.flows.electric_w.map_or(0.0, |e| e.signed_w());
                    let s = (p * p + q * q).sqrt();
                    // Zero apparent power has no power factor: the ratio is
                    // undefined, the cell null (today it carried the 1.0
                    // sentinel).
                    writer.write_opt(pf_idx, (s > 1e-9).then(|| (p / s).abs()));
                }
            }
            if let Some(idx) = cols.energy_kwh {
                let dt_s = inputs.env.time_step_secs();
                // Not nullable: the step's energy is zero when the
                // equipment drew nothing. The value is the step's J (SI);
                // the column's declared kWh unit converts once in the
                // writer.
                writer.write(
                    idx,
                    co.flows.electric_w.map_or(0.0, |e| e.net_consumption_w()) * dt_s,
                );
            }
            if let Some(idx) = cols.er_power {
                // OCHRE HVAC.py:1464-1467: ER Power = er_capacity * er_eir_rated * space_fraction / 1000.
                // Maps to BACKUP_ER_KW (already space-fraction-adjusted in heater step).
                writer.write(
                    idx,
                    eq.telemetry().get(tk::BACKUP_ER_KW).unwrap_or(0.0), // allowed: ER power remains telemetry-only until CoreOutput gains an er_power field.
                );
            }
            // Per-equipment HVAC performance columns at v7.
            // Some read from CoreOutput (setpoint, capacity, COP, speed, main_power),
            // the rest remain telemetry-only until their fields are promoted.
            if let Some(idx) = cols.shr {
                // The sensible heat ratio exists while the step delivered
                // sensible cooling: the cell is null otherwise (the
                // telemetry key holds the last cooling step's value, not a
                // per-step presence), never a 0.0 sentinel.
                writer.write_opt(
                    idx,
                    co.flows
                        .sensible_cooling_w
                        .filter(|sc| *sc < 0.0)
                        .and_then(|_| eq.telemetry().get(tk::SHR)), // allowed: SHR remains telemetry-only until CoreOutput gains an SHR field.
                );
            }
            if let Some(idx) = cols.fan_power {
                writer.write(
                    idx,
                    eq.telemetry().get(tk::FAN_KW).unwrap_or(0.0), // allowed: fan power remains telemetry-only until CoreOutput gains a fan power field.
                );
            }
            if let Some(idx) = cols.main_power {
                writer.write(idx, co.performance.main_power_w.unwrap_or(0.0));
            }
            if let Some(idx) = cols.runtime_fraction {
                writer.write(
                    idx,
                    eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(0.0), // allowed: runtime fraction remains telemetry-only until CoreOutput gains an RTF field.
                );
            }
            if let Some(idx) = cols.latent_gains {
                // OCHRE HVAC.py:595: Latent Gains = latent_gain * space_fraction (pre-DSE).
                writer.write(
                    idx,
                    eq.telemetry().get(tk::LATENT_GAINS_W).unwrap_or(0.0), // allowed: latent gains remains telemetry-only until CoreOutput gains a latent gains field.
                );
            }
            if let Some(idx) = cols.duct_losses {
                // Aggregate column: sum duct_loss_w across all HVAC equipment.
                // Only the first equipment's index is used; all HVAC contributions
                // are accumulated into the same row slot.
                writer.add(
                    idx,
                    eq.telemetry().get(tk::DUCT_LOSS_W).unwrap_or(0.0), // allowed: duct loss remains telemetry-only until CoreOutput gains a duct loss field.
                );
            }
            for &(key, idx) in &cols.v8_flow_columns {
                writer.write_opt(idx, eq.telemetry().get(key)); // allowed: v8 telemetry pass-through uses pre-resolved tk:: constants bound to `key`.
            }
        }

        // The slots whose value is the row's zero every step: the summed
        // cells' accumulator when its contributors exist, and the
        // not-nullable columns a roster change orphaned (see the
        // bindings' builder).
        for &slot in &self.bindings.always_valid_slots {
            writer.mark_valid(slot);
        }

        // Per-EndUse aggregate electric power columns.
        // Each equipment's electric power is accumulated into the aggregate column
        // for its EndUse category (e.g. all HVAC_HEATING equipment contribute to
        // "HVAC Heating End Use Electric Power (kW)").
        for (eq_idx, &agg_idx_opt) in self
            .bindings
            .end_use_aggregates
            .iter()
            .take(inputs.equipment.len())
            .enumerate()
        {
            if let Some(idx) = agg_idx_opt
                && !inputs.step_failed[eq_idx]
            {
                writer.add(
                    idx,
                    inputs.equipment[eq_idx]
                        .core_output()
                        .flows
                        .electric_w
                        .map_or(0.0, |e| e.net_consumption_w()),
                );
            }
        }

        // Actor telemetry columns: pre-resolved column indices avoid
        // per-timestep format!() allocation (parallel to the plan's
        // equipment bindings). A key the actor did not emit this step
        // leaves its cell unwritten, null.
        for (actor, pre_resolved) in inputs.actors.iter().zip(&self.bindings.actors) {
            if let Some(tel) = actor.telemetry() {
                for (key, column_idx) in pre_resolved {
                    if let Some(value) = tel.get(key) {
                        // The actors hold SI values; the declared unit's
                        // reporting column converts once in the writer.
                        writer.write(*column_idx, value);
                    }
                }
            }
        }

        // Zone temperature columns -- direct index lookup, no allocations.
        for (i, &(_, temp_c)) in inputs.zone_temps.iter().enumerate() {
            if let Some(idx) = self.zones.zone_temp_col_indices[i] {
                writer.write(idx, temp_c);
            }
        }
        // Fallback for old-style column name
        if let Some(idx) = dwelling_columns.indoor_temp_fallback
            && let Some(&(_, temp_c)) = inputs.zone_temps.first()
        {
            writer.write(idx, temp_c);
        }

        if let Some(idx) = dwelling_columns.outdoor_dry_bulb {
            writer.write(idx, inputs.env.weather.outdoor_temp_c);
        }

        // Envelope, boundary, and HVAC thermal gains, written through the
        // plan's frozen gain terms and column indices.
        let gains = inputs.gains;
        for &(term, idx) in &self.envelope_columns {
            if let Some(idx) = idx {
                writer.write(idx, term(gains));
            }
        }

        if let Some(idx) = dwelling_columns.ground_temp {
            writer.write(idx, inputs.env.weather.ground_temp_c);
        }
        if let Some(idx) = dwelling_columns.mains_temp {
            writer.write(idx, inputs.env.weather.mains_temp_c);
        }
        // The engine's per-step weather channels (the resample_weather
        // row): the run's internal series, the values the environment's
        // evaluation carried this step.
        if let Some(idx) = dwelling_columns.outdoor_humidity_ratio {
            writer.write(idx, inputs.env.weather.outdoor_humidity_ratio);
        }
        if let Some(idx) = dwelling_columns.pressure_kpa {
            writer.write(idx, inputs.env.weather.pressure_kpa);
        }
        if let Some(idx) = dwelling_columns.wind_speed_m_s {
            writer.write(idx, inputs.env.weather.wind_speed_m_s);
        }
        if let Some(idx) = dwelling_columns.sky_temp_c {
            writer.write(idx, inputs.env.weather.sky_temp_c);
        }
        if let Some(idx) = dwelling_columns.ghi_w_m2 {
            writer.write(idx, inputs.env.weather.ghi_w_m2);
        }
        if let Some(idx) = dwelling_columns.dni_w_m2 {
            writer.write(idx, inputs.env.weather.dni_w_m2);
        }
        if let Some(idx) = dwelling_columns.dhi_w_m2 {
            writer.write(idx, inputs.env.weather.dhi_w_m2);
        }

        // Setpoint chain: dwelling-level context columns for control auditing.
        // Read from the first HVAC equipment that publishes these keys; setpoint
        // reconciliation writes them to all HVAC equipment telemetry, so any
        // one carries the correct value. The equipment position is frozen
        // with the plan (every roster change rebuilds it). Before the first
        // reconcile the keys are absent and the cells stay unwritten, null.
        if let Some(first_hvac_pos) = dwelling_columns.first_hvac {
            let first_hvac = &inputs.equipment[first_hvac_pos];
            if let Some(idx) = dwelling_columns.scheduled_heating_setpoint {
                writer.write_opt(
                    idx,
                    // allowed: setpoint telemetry keys are populated by HVAC
                    // equipment reconcile logic after the first timestep; before
                    // that the cell stays unwritten, null.
                    first_hvac.telemetry().get(tk::SCHEDULE_HEATING_SETPOINT_C),
                );
            }
            if let Some(idx) = dwelling_columns.scheduled_cooling_setpoint {
                writer.write_opt(
                    idx,
                    // allowed: see the scheduled heating setpoint read above.
                    first_hvac.telemetry().get(tk::SCHEDULE_COOLING_SETPOINT_C),
                );
            }
            if let Some(idx) = dwelling_columns.runtime_heating_setpoint {
                writer.write_opt(
                    idx,
                    // allowed: see the scheduled heating setpoint read above.
                    first_hvac.telemetry().get(tk::RUNTIME_HEATING_SETPOINT_C),
                );
            }
            if let Some(idx) = dwelling_columns.runtime_cooling_setpoint {
                writer.write_opt(
                    idx,
                    // allowed: see the scheduled heating setpoint read above.
                    first_hvac.telemetry().get(tk::RUNTIME_COOLING_SETPOINT_C),
                );
            }
        }

        for &(zone, value) in &gains.infiltration_by_zone {
            if let Some(&idx) = self.zones.infiltration_columns.get(&zone) {
                writer.write(idx, value);
            }
        }

        for &(zone, value) in &gains.interior_lwr_by_zone {
            if let Some(&idx) = self.zones.lwr_columns.get(&zone) {
                writer.write(idx, value);
            }
        }

        // Per-zone HVAC thermal attribution: read sensible gains by category
        // from the thermal port accumulators. Ports still hold equipment-step
        // values because ports.zero() runs AFTER record_step.
        // The basement zone receives HvacHeating/HvacCooling directly
        // (not merged into conditioned); duct zones are tagged DuctLoss which
        // is reported in the separate "HVAC Duct Losses (W)" column at
        // verbosity 5, not here.
        for thermal in &inputs.ports.thermal {
            if let Some(&(heat_idx, cool_idx)) = self.zones.hvac_columns.get(&thermal.zone) {
                writer.write(
                    heat_idx,
                    thermal.sensible_for_category(hares_types::ThermalCategory::HvacHeating),
                );
                writer.write(
                    cool_idx,
                    thermal
                        .sensible_for_category(hares_types::ThermalCategory::HvacCooling)
                        .abs(),
                );
            }
        }

        // The diagnostics block: written only when the run requested the
        // diagnostics report and the frozen schema carries the block. Every
        // value is the per-step diagnostics CSV's row semantics (the zone
        // lookups default to 0.0 the same way), at full f64 precision.
        if let Some(diag) = &self.bindings.diagnostics {
            write_diagnostics_row(
                writer,
                diag,
                inputs.step_number,
                inputs.env,
                inputs.ports,
                gains,
            );
        }

        // Every report's recorder gets the row its selection projected out
        // of the union row, in the report's own column order. The
        // projections were copied at freeze; the union row's borrow ends
        // with the producers' writes above.
        // The run metrics' one path: the accumulator is fed per step from
        // the row plan's producer slots, before anything reads the row
        // onward. `None` when the run did not request metrics: the step
        // pays one branch and nothing else.
        if let Some(calculator) = metrics {
            calculator.accumulate_row(writer.row()).map_err(|err| {
                HaresError::InvalidState(format!("metrics accumulation failed: {err}"))
            })?;
        }
        for (recorder, projection) in recorders.iter_mut().zip(self.projections.iter()) {
            self.projected_row_buf.clear();
            self.projected_row_buf
                .extend(projection.iter().map(|&slot| writer.row()[slot]));
            self.projected_valid_buf.clear();
            self.projected_valid_buf
                .extend(projection.iter().map(|&slot| writer.valid(slot)));
            if let Err(err) =
                recorder.push_row(time_ms, &self.projected_row_buf, &self.projected_valid_buf)
            {
                return Err(recorder_error("record push", err));
            }
        }
        Ok(())
    }
}

/// Writes one step's diagnostics columns into the plan row: the zone
/// temperatures, the thermal ports' sensible and latent gains, the
/// electrical net from the ports, and the thermal solver's component
/// gains. The values are the per-step diagnostics CSV's row semantics
/// (a zone absent from the environment or the ports records 0.0, the same
/// default the CSV's row writer applied), at full f64 precision. Every
/// bound slot is written, valid every step.
fn write_diagnostics_row(
    writer: &mut RowWriter<'_>,
    diag: &DiagnosticsBindings,
    step: u64,
    env: &EnvironmentState,
    ports: &PortSlots,
    gains: &EnvelopeComponentGains,
) {
    writer.write(diag.step, step as f64);
    writer.write(diag.timestamp_s, env.current_time.timestamp() as f64);
    writer.write(diag.outdoor_temp_c, env.weather.outdoor_temp_c);
    for zone in &diag.zones {
        let temp = env
            .zones
            .iter()
            .find(|z| z.id == zone.zone)
            .map(|z| z.temperature_c)
            .unwrap_or(0.0);
        writer.write(zone.temp_c, temp);
        let thermal = ports.thermal.iter().find(|t| t.zone == zone.zone);
        writer.write(
            zone.thermal_gain_w,
            thermal.map_or(0.0, |t| t.heat().sensible_w()),
        );
        writer.write(
            zone.latent_gain_w,
            thermal.map_or(0.0, |t| t.heat().latent_w),
        );
    }
    // The diagnostics CSV's reporting names convert once from SI here.
    writer.write(diag.electrical_net_w, ports.electrical.net_active_w());
    writer.write(diag.electrical_net_var, ports.electrical.reactive_power_var);
    writer.write(diag.envelope.port_radiant_w, gains.port_radiant_w);
    writer.write(diag.envelope.port_shortwave_w, gains.port_shortwave_w);
    writer.write(diag.envelope.port_convective_w, gains.port_convective_w);
    writer.write(diag.envelope.window_solar_w, gains.window_solar_w);
    writer.write(diag.envelope.opaque_solar_lwr_w, gains.opaque_solar_lwr_w);
    writer.write(diag.envelope.interior_lwr_w, gains.interior_lwr_w);
    writer.write(diag.envelope.internal_gain_w, gains.internal_gain_w);
    writer.write(diag.envelope.air_density_kg_m3, gains.air_density_kg_m3);
    writer.write(diag.envelope.ghi_w_m2, env.weather.ghi_w_m2);
}

/// The recorder error the run's step path maps: a callback failure
/// keeps its typed variant (the run names the report and stays ended),
/// every other recorder failure is an I/O error as before, prefixed
/// with the flushing context that produced it.
pub(super) fn recorder_error(context: &str, err: OutputError) -> HaresError {
    match err {
        OutputError::Callback { report, message } => HaresError::Callback { report, message },
        other => HaresError::Io(format!("{context} failed: {other}")),
    }
}
