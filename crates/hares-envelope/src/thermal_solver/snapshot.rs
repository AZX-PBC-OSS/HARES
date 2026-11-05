//! Checkpoint snapshot and validated, atomic restore of the thermal solver's
//! mutable state.

use std::collections::HashMap;

use hares_types::ZoneId;
use nalgebra::DVector;
use serde::{Deserialize, Serialize};

use super::ThermalSolver;
use super::config::{Result, ThermalSolverError};
use crate::state_space::ZERO_GAIN_EPSILON;

/// The schema version of the serialized [`ThermalSnapshot`], independent of
/// the dwelling checkpoint's `CHECKPOINT_VERSION`: branches whose checkpoint
/// schema changes share a `CHECKPOINT_VERSION` reconcile it with one bump at
/// their fold, while this version tracks this struct's own layout.
///
/// v1: the ventilation recovery effectiveness the next step reads, the
/// ideal-capacity failure counts and last-good capacities its degraded
/// fallback reads, and this field itself joined the snapshot. A snapshot
/// without this field is from before the versioning and fails to
/// deserialize.
///
/// v2: the step's active ventilation-flow variant, which the next step's
/// ideal-capacity solve reads before the envelope re-selects it. The
/// schedule-driven selection is mutable solver state the first post-restore
/// solve depends on, so a snapshot without it handed that solve the
/// rebuilding solver's selection instead of the written run's.
///
/// v3: the mechanical ventilation forced flow the next step's coupling
/// reads, as last set by [`ThermalSolver::set_ventilation_flow`]. The
/// first post-restore ideal-capacity solve reads it for the same reason
/// the recovery efficiencies ride the snapshot: the propagation from the
/// equipment runs inside a step, and a restore lands between steps.
pub const THERMAL_SNAPSHOT_SCHEMA_VERSION: u32 = 3;

/// Captured thermal state for checkpoint save/restore: every piece of
/// mutable solver state that a step's or an ideal-capacity solve's result
/// depends on. The solver's remaining mutable state is per-step scratch
/// rebuilt before it is read, or log throttling and the per-solve degraded
/// flags, which [`ThermalSolver::restore_state`] resets.
///
/// Its serialized form is part of the dwelling checkpoint schema
/// (`hares_core::checkpoint::DwellingCheckpoint`): a layout change bumps
/// [`THERMAL_SNAPSHOT_SCHEMA_VERSION`] here and is folded into the next
/// `CHECKPOINT_VERSION` bump there, which a schema test in that crate
/// enforces. When branches whose checkpoint changes share the current
/// `CHECKPOINT_VERSION` fold, that version is bumped once to a fresh value
/// carrying every change, and this struct's own version is unaffected, so
/// the reconciliation is trivial.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThermalSnapshot {
    /// The [`THERMAL_SNAPSHOT_SCHEMA_VERSION`] the snapshot was written
    /// with, validated against this build's value before any restore.
    pub schema_version: u32,
    pub x: Vec<f64>,
    pub last_u: Vec<f64>,
    pub lwr_t_prev_c: Vec<f64>,
    pub interior_surface_temps: Vec<Vec<f64>>,
    pub interior_surface_prev_temps: Vec<Vec<f64>>,
    /// Coupling tuples `(state_idx, d_implicit, forcing)` of the last
    /// prepared or integrated step.
    pub last_coupling: Vec<(usize, f64, f64)>,
    /// The last step's active ventilation-flow model variant (an index into
    /// the configured variants), or `None` when the base model governed.
    /// The next step's ideal-capacity solve runs before the envelope
    /// re-selects the variant, so this is the model that solve reads.
    pub active_flow_model: Option<usize>,
    /// Mechanical ventilation sensible recovery effectiveness the next step
    /// reads, as last set by [`ThermalSolver::set_ventilation_recovery`].
    pub sensible_recovery_efficiency: f64,
    /// Latent counterpart of `sensible_recovery_efficiency`.
    pub latent_recovery_efficiency: f64,
    /// Mechanical ventilation forced flow (m³/s) the next step's coupling
    /// reads, as last set by [`ThermalSolver::set_ventilation_flow`].
    pub ventilation_flow_m3_s: f64,
    /// Consecutive ideal-capacity solve failures per zone, sorted by zone.
    pub ideal_capacity_failure_counts: Vec<(ZoneId, usize)>,
    /// Last successfully solved ideal capacity (W) per zone, sorted by zone:
    /// the degraded fallback once failures reach the threshold.
    pub last_good_capacity_w: Vec<(ZoneId, f64)>,
    /// Per-zone non-HVAC share of the zone sensible input column (W) from
    /// the last integrate, sorted by zone: the estimate the next
    /// ideal-capacity solve subtracts.
    pub non_hvac_zone_input_w: Vec<(ZoneId, f64)>,
}

/// Every coupling's forcing is its implicit coefficient `d` times a
/// reference temperature: `d·(T_drive + x_i)` for infiltration and exterior
/// longwave, `d·T_other` for interior convection, each term an envelope
/// temperature [°C]. A restored coupling whose implied reference
/// `|forcing| / d` exceeds this bound cannot come from envelope
/// temperatures and drives the coupled solve toward overflow; within it,
/// the coupled update `(forcing − d·x_i)/(1 + d)` stays within the
/// reference and state magnitudes.
const MAX_COUPLING_REFERENCE_TEMPERATURE_C: f64 = 1.0e4;

pub(super) fn validate_recovery_efficiencies(sensible: f64, latent: f64) -> Result<()> {
    for (kind, value) in [("sensible", sensible), ("latent", latent)] {
        if !(0.0..=1.0).contains(&value) {
            return Err(ThermalSolverError::InvalidVentilationRecovery { kind, value });
        }
    }
    Ok(())
}

/// Sorted `(zone, value)` pairs of a per-zone map.
fn sorted_zone_pairs<V: Copy>(map: &HashMap<ZoneId, V>) -> Vec<(ZoneId, V)> {
    let mut pairs: Vec<(ZoneId, V)> = map.iter().map(|(&zone, &v)| (zone, v)).collect();
    pairs.sort_unstable_by_key(|&(zone, _)| zone);
    pairs
}

fn invalid(detail: String) -> ThermalSolverError {
    ThermalSolverError::InvalidSnapshot(detail)
}

fn check_len(field: &str, got: usize, expected: usize) -> Result<()> {
    if got == expected {
        Ok(())
    } else {
        Err(invalid(format!(
            "{field} length: got {got}, expected {expected}"
        )))
    }
}

fn check_finite(field: &str, values: &[f64]) -> Result<()> {
    match values.iter().position(|v| !v.is_finite()) {
        None => Ok(()),
        Some(i) => Err(invalid(format!("non-finite {field}[{i}]: {}", values[i]))),
    }
}

fn check_nested(field: &str, got: &[Vec<f64>], expected: &[Vec<f64>]) -> Result<()> {
    check_len(field, got.len(), expected.len())?;
    for (i, (zone, live)) in got.iter().zip(expected).enumerate() {
        let zone_field = format!("{field}[{i}]");
        check_len(&zone_field, zone.len(), live.len())?;
        check_finite(&zone_field, zone)?;
    }
    Ok(())
}

impl ThermalSolver {
    /// Captures the solver's checkpointable state.
    #[must_use]
    pub fn snapshot_state(&self) -> ThermalSnapshot {
        ThermalSnapshot {
            schema_version: THERMAL_SNAPSHOT_SCHEMA_VERSION,
            x: self.x.iter().copied().collect(),
            last_u: self.last_u.iter().copied().collect(),
            lwr_t_prev_c: self.exterior_surface_temps.clone(),
            interior_surface_temps: self.interior_surface_temps.clone(),
            interior_surface_prev_temps: self.interior_surface_prev_temps.clone(),
            last_coupling: self.last_coupling.clone(),
            active_flow_model: self.active_flow_model,
            sensible_recovery_efficiency: self.config.ventilation.sensible_recovery_efficiency,
            latent_recovery_efficiency: self.config.ventilation.latent_recovery_efficiency,
            ventilation_flow_m3_s: self.config.ventilation_flow_m3_s,
            ideal_capacity_failure_counts: sorted_zone_pairs(&self.ideal_capacity_failure_counts),
            last_good_capacity_w: sorted_zone_pairs(&self.last_good_capacity_w),
            non_hvac_zone_input_w: sorted_zone_pairs(&self.non_hvac_zone_input_w),
        }
    }

    /// Checks that `snap` describes a state this solver can hold: its
    /// `schema_version` is this build's, every vector has this solver's
    /// dimensions and finite entries, every coupling keeps the coupled solve
    /// finite and leaves every zone's ideal-capacity gain as solvable as the
    /// uncoupled model makes it, the recovery efficiencies are in `[0, 1]`,
    /// the active flow variant names one of this solver's configured
    /// variants, and the fallback state names only zones this solver
    /// solves, in zone order. Reads the solver only.
    pub fn validate_snapshot(&self, snap: &ThermalSnapshot) -> Result<()> {
        if snap.schema_version != THERMAL_SNAPSHOT_SCHEMA_VERSION {
            return Err(invalid(format!(
                "schema version: got {}, expected {}",
                snap.schema_version, THERMAL_SNAPSHOT_SCHEMA_VERSION
            )));
        }
        if let Some(idx) = snap.active_flow_model
            && idx >= self.config.ventilation.flow_models.len()
        {
            return Err(invalid(format!(
                "active flow model {idx} is not one of this solver's {} configured \
                 ventilation-flow variants",
                self.config.ventilation.flow_models.len()
            )));
        }
        check_len("x", snap.x.len(), self.x.len())?;
        check_finite("x", &snap.x)?;
        check_len("last_u", snap.last_u.len(), self.last_u.len())?;
        check_finite("last_u", &snap.last_u)?;
        check_len(
            "lwr_t_prev_c",
            snap.lwr_t_prev_c.len(),
            self.exterior_surface_temps.len(),
        )?;
        check_finite("lwr_t_prev_c", &snap.lwr_t_prev_c)?;
        check_nested(
            "interior_surface_temps",
            &snap.interior_surface_temps,
            &self.interior_surface_temps,
        )?;
        check_nested(
            "interior_surface_prev_temps",
            &snap.interior_surface_prev_temps,
            &self.interior_surface_prev_temps,
        )?;
        self.validate_couplings(&snap.last_coupling)?;
        validate_recovery_efficiencies(
            snap.sensible_recovery_efficiency,
            snap.latent_recovery_efficiency,
        )?;
        if !snap.ventilation_flow_m3_s.is_finite() || snap.ventilation_flow_m3_s < 0.0 {
            return Err(invalid(format!(
                "ventilation_flow_m3_s {} must be finite and >= 0",
                snap.ventilation_flow_m3_s
            )));
        }
        self.validate_zone_pairs(
            "ideal_capacity_failure_counts",
            &snap.ideal_capacity_failure_counts,
            |&count| count > 0,
        )?;
        self.validate_zone_pairs("last_good_capacity_w", &snap.last_good_capacity_w, |w| {
            w.is_finite()
        })?;
        self.validate_zone_pairs("non_hvac_zone_input_w", &snap.non_hvac_zone_input_w, |w| {
            w.is_finite()
        })
    }

    fn validate_couplings(&self, couplings: &[(usize, f64, f64)]) -> Result<()> {
        let n = self.x.len();
        let mut d_agg = DVector::<f64>::zeros(n);
        for (i, &(state_idx, d, forcing)) in couplings.iter().enumerate() {
            let in_range = state_idx < n
                && d.is_finite()
                && d >= 0.0
                && forcing.is_finite()
                && forcing.abs() <= d * MAX_COUPLING_REFERENCE_TEMPERATURE_C;
            if !in_range {
                return Err(invalid(format!(
                    "last_coupling[{i}] = ({state_idx}, {d}, {forcing}): the state index \
                     must be below {n}, the coefficient d finite and non-negative, and \
                     |forcing| at most d × {MAX_COUPLING_REFERENCE_TEMPERATURE_C} °C"
                )));
            }
            d_agg[state_idx] += d;
        }
        if let Some(i) = d_agg.iter().position(|d| !d.is_finite()) {
            return Err(invalid(format!(
                "last_coupling aggregates to a non-finite coefficient at state {i}"
            )));
        }

        let no_coupling = DVector::zeros(n);
        let mut gain = DVector::zeros(n);
        for (&zone, &input_idx) in &self.wiring.zone_sensible_input_indices {
            let Some(&output_idx) = self.wiring.zone_output_indices.get(&zone) else {
                continue;
            };
            let uncoupled = self.model.identity_coupled_effective_gain(
                output_idx,
                input_idx,
                &no_coupling,
                &mut gain,
            );
            let coupled = self
                .model
                .identity_coupled_effective_gain(output_idx, input_idx, &d_agg, &mut gain);
            if uncoupled.abs() > ZERO_GAIN_EPSILON && coupled.abs() <= ZERO_GAIN_EPSILON {
                return Err(invalid(format!(
                    "last_coupling damps zone {zone:?}'s ideal-capacity gain to {coupled}, \
                     which no solve can divide by (uncoupled gain {uncoupled})"
                )));
            }
        }
        Ok(())
    }

    fn validate_zone_pairs<V: std::fmt::Debug>(
        &self,
        field: &str,
        pairs: &[(ZoneId, V)],
        valid: impl Fn(&V) -> bool,
    ) -> Result<()> {
        for (i, (zone, value)) in pairs.iter().enumerate() {
            if !self.wiring.zone_sensible_input_indices.contains_key(zone) {
                return Err(invalid(format!(
                    "{field}[{i}] names zone {zone:?}, which this solver does not solve"
                )));
            }
            if !valid(value) {
                return Err(invalid(format!(
                    "{field}[{i}] = {value:?} for zone {zone:?}"
                )));
            }
            if i > 0 && pairs[i - 1].0 >= *zone {
                return Err(invalid(format!(
                    "{field} is not in strictly increasing zone order at index {i}"
                )));
            }
        }
        Ok(())
    }

    /// Restores the solver's checkpointable state from `snap`.
    ///
    /// [`Self::validate_snapshot`] runs before any mutation, so the solver
    /// is left unchanged when the snapshot is invalid (atomic restore). The
    /// log-throttling sets and the per-solve degraded flags are cleared:
    /// they gate repeated log lines and describe the most recent solve, and
    /// a restored solver has not solved yet, so its first failing solve
    /// logs at warn level again.
    pub fn restore_state(&mut self, snap: &ThermalSnapshot) -> Result<()> {
        self.validate_snapshot(snap)?;

        self.invalidate_shared_prefix();
        self.active_flow_model = snap.active_flow_model;
        self.x.copy_from_slice(&snap.x);
        self.last_u.copy_from_slice(&snap.last_u);
        self.last_coupling.clone_from(&snap.last_coupling);
        self.exterior_surface_temps
            .copy_from_slice(&snap.lwr_t_prev_c);
        for (live, saved) in self
            .interior_surface_temps
            .iter_mut()
            .zip(&snap.interior_surface_temps)
        {
            live.copy_from_slice(saved);
        }
        for (live, saved) in self
            .interior_surface_prev_temps
            .iter_mut()
            .zip(&snap.interior_surface_prev_temps)
        {
            live.copy_from_slice(saved);
        }
        self.config.ventilation.sensible_recovery_efficiency = snap.sensible_recovery_efficiency;
        self.config.ventilation.latent_recovery_efficiency = snap.latent_recovery_efficiency;
        self.config.ventilation_flow_m3_s = snap.ventilation_flow_m3_s;
        self.ideal_capacity_failure_counts.clear();
        self.ideal_capacity_failure_counts
            .extend(snap.ideal_capacity_failure_counts.iter().copied());
        self.last_good_capacity_w.clear();
        self.last_good_capacity_w
            .extend(snap.last_good_capacity_w.iter().copied());
        self.non_hvac_zone_input_w.clear();
        self.non_hvac_zone_input_w
            .extend(snap.non_hvac_zone_input_w.iter().copied());
        self.ideal_capacity_degraded_zones.clear();
        self.ideal_capacity_warned_zones.clear();
        self.ideal_capacity_degraded_warned_zones.clear();
        Ok(())
    }
}
