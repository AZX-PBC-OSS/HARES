//! The post-hoc diagnostic checks over the run's accumulated per-step data.
//!
//! The per-step diagnostics are a catalogue report: the run requests them
//! and their columns are recorded with the run's frames. On top of the
//! recorded columns, when `cfg(feature = "observe")` is active,
//! `DiagnosticAccumulator` collects per-step data during the simulation and
//! `run_post_hoc_checks()` evaluates four diagnostic checks at end-of-run:
//!
//! - Excessive unmet heating/cooling hours
//! - Equipment short-cycling (mode change frequency)
//! - Temperature excursions below freezing in conditioned zones
//! - Simultaneous heating and cooling in the same zone
//!
//! Thresholds follow the defaults in the ticket:
//! `unmet_load_threshold = 0.05` (5%), `max_cycles_per_hour = 4`.

#[cfg(feature = "observe")]
use hares_equipment::Equipment;
#[cfg(feature = "observe")]
use hares_types::{OperatingMode, ZoneId, ZoneState};

/// Per-zone counters accumulated across the simulation for post-hoc checks.
#[cfg(feature = "observe")]
#[derive(Debug, Clone)]
struct ZoneDiagnosticCounters {
    zone_id: ZoneId,
    is_conditioned: bool,
    unmet_heating_steps: u64,
    unmet_cooling_steps: u64,
    freezing_steps: u64,
    steps_with_setpoint: u64,
}

/// Per-equipment counters accumulated across the simulation for post-hoc checks.
#[cfg(feature = "observe")]
#[derive(Debug, Clone)]
struct EquipmentDiagnosticCounters {
    name: String,
    zone_id: Option<ZoneId>,
    mode_changes: u64,
}

/// Threshold defaults from the ticket:
///
/// - `UNMET_LOAD_THRESHOLD = 0.05` (5%): maximum fraction of occupied
///   hours that zone temperature may be outside setpoint bounds.
/// - `MAX_CYCLES_PER_HOUR = 4`: maximum mode-change events per equipment
///   per simulation hour before short-cycling is flagged.
#[cfg(feature = "observe")]
const UNMET_LOAD_THRESHOLD: f64 = 0.05;

#[cfg(feature = "observe")]
const MAX_CYCLES_PER_HOUR: f64 = 4.0;

/// Accumulates per-step diagnostic data for end-of-run post-hoc checks.
///
/// # Invariants
/// - `zone_counters.len()` must match the number of zones in the dwelling.
/// - `equipment_counters.len()` must match `equipment.len()`.
/// - `simultaneous_hc_steps.len()` must match `zone_counters.len()`.
/// - `prev_equipment_modes.len()` must match `equipment_counters.len()`.
#[cfg(feature = "observe")]
#[derive(Debug, Clone)]
pub struct DiagnosticAccumulator {
    zone_counters: Vec<ZoneDiagnosticCounters>,
    equipment_counters: Vec<EquipmentDiagnosticCounters>,
    prev_equipment_modes: Vec<Option<OperatingMode>>,
    total_steps: u64,
    simultaneous_hc_steps: Vec<u64>,
    zone_has_heating: Vec<bool>,
    zone_has_cooling: Vec<bool>,
    /// Pre-allocated scratch buffers, reused per timestep to avoid heap
    /// allocation in the hot loop.
    scratch: StepScratch,
}

/// Per-step inputs [`DiagnosticAccumulator::record_from_state`] extracts
/// from the dwelling's state before recording them.
#[cfg(feature = "observe")]
#[derive(Debug, Clone, Default)]
struct StepScratch {
    zone_temps: Vec<f64>,
    heating_sps: Vec<Option<f64>>,
    cooling_sps: Vec<Option<f64>>,
    equipment_modes: Vec<Option<OperatingMode>>,
    equipment_zone_indices: Vec<Option<usize>>,
}

#[cfg(feature = "observe")]
impl DiagnosticAccumulator {
    /// Creates a new accumulator for a dwelling with the given zone and equipment
    /// configuration.
    ///
    /// `zone_ids` and `zone_conditioned` must be in lockstep: the i-th entry
    /// in `zone_conditioned` corresponds to the i-th entry in `zone_ids`.
    /// `equipment_names` and `equipment_zones` must be in lockstep.
    pub fn new(
        zone_ids: &[ZoneId],
        zone_conditioned: &[bool],
        equipment_names: &[String],
        equipment_zones: &[Option<ZoneId>],
    ) -> Self {
        let zone_counters = zone_ids
            .iter()
            .zip(zone_conditioned.iter())
            .map(|(id, cond)| ZoneDiagnosticCounters {
                zone_id: *id,
                is_conditioned: *cond,
                unmet_heating_steps: 0,
                unmet_cooling_steps: 0,
                freezing_steps: 0,
                steps_with_setpoint: 0,
            })
            .collect();
        let equipment_counters = equipment_names
            .iter()
            .zip(equipment_zones.iter())
            .map(|(name, zid)| EquipmentDiagnosticCounters {
                name: name.clone(),
                zone_id: *zid,
                mode_changes: 0,
            })
            .collect();
        let nz = zone_ids.len();
        let ne = equipment_names.len();
        Self {
            zone_counters,
            equipment_counters,
            prev_equipment_modes: vec![None; ne],
            total_steps: 0,
            simultaneous_hc_steps: vec![0; nz],
            zone_has_heating: vec![false; nz],
            zone_has_cooling: vec![false; nz],
            scratch: StepScratch {
                zone_temps: vec![0.0; nz],
                heating_sps: vec![None; nz],
                cooling_sps: vec![None; nz],
                equipment_modes: vec![None; ne],
                equipment_zone_indices: vec![None; ne],
            },
        }
    }

    /// Records one step from the dwelling's live state, using pre-allocated
    /// scratch buffers to avoid per-timestep heap allocation.
    ///
    /// Extracts zone temperatures, heating/cooling setpoints, equipment
    /// operating modes, and equipment zone indices from the dwelling's zone
    /// list and equipment list, then delegates to [`Self::record_step`]. The
    /// scratch is moved out for the call and back afterwards, which moves
    /// only the vectors' handles.
    pub fn record_from_state(&mut self, env_zones: &[ZoneState], equipment: &[Box<dyn Equipment>]) {
        let mut scratch = std::mem::take(&mut self.scratch);
        let nz = env_zones.len().min(scratch.zone_temps.len());
        let ne = equipment.len().min(scratch.equipment_modes.len());

        // --- Zone temperatures ---
        scratch.zone_temps[..nz].fill(0.0);
        for (i, z) in env_zones.iter().take(nz).enumerate() {
            scratch.zone_temps[i] = z.temperature_c;
        }

        // --- Heating / cooling setpoints ---
        scratch.heating_sps[..nz].fill(None);
        scratch.cooling_sps[..nz].fill(None);
        for eq in equipment.iter() {
            let co = eq.core_output();
            let Some(zone_id) = eq.descriptor().zone else {
                continue;
            };
            let Some(zone_idx) = env_zones.iter().position(|z| z.id == zone_id) else {
                continue;
            };
            if zone_idx >= nz {
                continue;
            }
            if let Some(sp) = co.state.setpoint_c {
                match co.state.operating_mode {
                    Some(
                        OperatingMode::Heating
                        | OperatingMode::HeatingHP
                        | OperatingMode::HeatingER
                        | OperatingMode::HeatingHPAndER,
                    ) => scratch.heating_sps[zone_idx] = Some(sp),
                    Some(OperatingMode::Cooling) => scratch.cooling_sps[zone_idx] = Some(sp),
                    _ => {}
                }
            }
        }

        // --- Equipment operating modes ---
        scratch.equipment_modes[..ne].fill(None);
        for (i, eq) in equipment.iter().take(ne).enumerate() {
            scratch.equipment_modes[i] = eq.core_output().state.operating_mode;
        }

        // --- Equipment → zone index mapping ---
        scratch.equipment_zone_indices[..ne].fill(None);
        for (i, eq) in equipment.iter().take(ne).enumerate() {
            scratch.equipment_zone_indices[i] = eq
                .descriptor()
                .zone
                .and_then(|zid| env_zones.iter().position(|z| z.id == zid));
        }

        self.record_step(
            &scratch.zone_temps[..nz],
            &scratch.heating_sps[..nz],
            &scratch.cooling_sps[..nz],
            &scratch.equipment_modes[..ne],
            &scratch.equipment_zone_indices[..ne],
        );
        self.scratch = scratch;
    }

    /// Runs all four post-hoc diagnostic checks using accumulated data and
    /// emits structured `tracing` warnings/errors for the findings.
    ///
    /// `time_res_s` is the timestep resolution in seconds, used to convert
    /// step counts to hours for the short-cycling check.
    ///
    /// Returns the number of diagnostic violations found (for caller observability).
    pub fn run_post_hoc_checks(&self, zone_names: &[String], time_res_s: f64) -> usize {
        let mut violations = 0;
        let sim_hours = (self.total_steps as f64) * time_res_s / 3600.0;
        if sim_hours <= 0.0 {
            return 0;
        }

        // -- Check 1: Excessive unmet hours --
        for zc in &self.zone_counters {
            if zc.steps_with_setpoint == 0 || !zc.is_conditioned {
                continue;
            }
            let denom = zc.steps_with_setpoint.max(1) as f64;
            let heat_pct = zc.unmet_heating_steps as f64 / denom * 100.0;
            let cool_pct = zc.unmet_cooling_steps as f64 / denom * 100.0;
            let threshold_pct = UNMET_LOAD_THRESHOLD * 100.0;

            if heat_pct > threshold_pct {
                let zone_name = zone_name_for(zc.zone_id, zone_names);
                tracing::warn!(
                    zone = %zone_name,
                    unmet_heating_pct = heat_pct,
                    threshold_pct = threshold_pct,
                    unmet_heating_steps = zc.unmet_heating_steps,
                    total_sensor_steps = zc.steps_with_setpoint,
                    "diagnostic: excessive unmet heating hours"
                );
                violations += 1;
            }
            if cool_pct > threshold_pct {
                let zone_name = zone_name_for(zc.zone_id, zone_names);
                tracing::warn!(
                    zone = %zone_name,
                    unmet_cooling_pct = cool_pct,
                    threshold_pct = threshold_pct,
                    unmet_cooling_steps = zc.unmet_cooling_steps,
                    total_sensor_steps = zc.steps_with_setpoint,
                    "diagnostic: excessive unmet cooling hours"
                );
                violations += 1;
            }
        }

        // -- Check 2: Equipment short-cycling --
        for ec in &self.equipment_counters {
            let cycles_per_hour = ec.mode_changes as f64 / sim_hours;
            if cycles_per_hour > MAX_CYCLES_PER_HOUR {
                let zone_label = ec
                    .zone_id
                    .map(|zid| zone_name_for(zid, zone_names))
                    .unwrap_or_else(|| "unassigned".to_string());
                tracing::warn!(
                    equipment = %ec.name,
                    zone = %zone_label,
                    mode_changes = ec.mode_changes,
                    sim_hours = sim_hours,
                    cycles_per_hour = cycles_per_hour,
                    max_cycles_per_hour = MAX_CYCLES_PER_HOUR,
                    "diagnostic: equipment short-cycling detected"
                );
                violations += 1;
            }
        }

        // -- Check 3: Freezing excursions in conditioned zones --
        for zc in &self.zone_counters {
            if zc.freezing_steps > 0 && zc.is_conditioned {
                let zone_name = zone_name_for(zc.zone_id, zone_names);
                tracing::error!(
                    zone = %zone_name,
                    freezing_steps = zc.freezing_steps,
                    total_steps = self.total_steps,
                    "diagnostic: temperature excursion below freezing in conditioned zone"
                );
                violations += 1;
            }
        }

        // -- Check 4: Simultaneous heating and cooling --
        for (zidx, count) in self.simultaneous_hc_steps.iter().enumerate() {
            if *count > 0 {
                let zc = &self.zone_counters[zidx];
                let zone_name = zone_name_for(zc.zone_id, zone_names);
                tracing::warn!(
                    zone = %zone_name,
                    simultaneous_hc_steps = count,
                    total_steps = self.total_steps,
                    "diagnostic: simultaneous heating and cooling detected in conditioned zone"
                );
                violations += 1;
            }
        }

        violations
    }
}

#[cfg(feature = "observe")]
impl DiagnosticAccumulator {
    /// Records one step of data. Call once per timestep when the run
    /// requested the diagnostics.
    ///
    /// - `zone_temps_c`, `heating_setpoints_c`, `cooling_setpoints_c` are all
    ///   indexed by zone position (not ZoneId).
    /// - `equipment_modes` and `equipment_zone_indices` are indexed by
    ///   equipment position. `equipment_zone_indices` maps each equipment to
    ///   the position of its zone in the zone arrays (or `None` if unassigned).
    pub fn record_step(
        &mut self,
        zone_temps_c: &[f64],
        heating_setpoints_c: &[Option<f64>],
        cooling_setpoints_c: &[Option<f64>],
        equipment_modes: &[Option<OperatingMode>],
        equipment_zone_indices: &[Option<usize>],
    ) {
        let Self {
            zone_counters,
            equipment_counters,
            prev_equipment_modes,
            total_steps,
            simultaneous_hc_steps,
            zone_has_heating,
            zone_has_cooling,
            scratch: _,
        } = self;
        *total_steps += 1;

        // --- Zone-level checks ---
        for (i, zc) in zone_counters.iter_mut().enumerate() {
            let t = zone_temps_c.get(i).copied().unwrap_or(f64::NAN);
            if !t.is_finite() {
                continue;
            }
            // Freezing check (conditioned zones only).
            if zc.is_conditioned && t < 0.0 {
                zc.freezing_steps += 1;
            }
            // Unmet hours: zone temperature vs setpoint bounds.
            let hsp = heating_setpoints_c.get(i).copied().flatten();
            let csp = cooling_setpoints_c.get(i).copied().flatten();
            let has_setpoint = hsp.is_some() || csp.is_some();
            if has_setpoint {
                zc.steps_with_setpoint += 1;
                if let Some(h) = hsp
                    && t < h
                {
                    zc.unmet_heating_steps += 1;
                }
                if let Some(c) = csp
                    && t > c
                {
                    zc.unmet_cooling_steps += 1;
                }
            }
        }

        // --- Equipment-level checks ---
        for (i, ec) in equipment_counters.iter_mut().enumerate() {
            let mode = equipment_modes.get(i).copied().flatten();
            // Detect mode changes: only transitions between consecutive known
            // states count.  Entering an active mode on the first step is the
            // initial state, not a cycling event.
            if let (Some(prev), Some(curr)) = (prev_equipment_modes.get(i).copied().flatten(), mode)
                && prev != curr
            {
                ec.mode_changes += 1;
            }

            prev_equipment_modes[i] = mode;
        }

        // --- Simultaneous heating + cooling detection ---
        let n_zones = zone_counters.len();
        zone_has_heating.fill(false);
        zone_has_cooling.fill(false);
        for (i, mode) in equipment_modes.iter().enumerate() {
            let Some(zidx) = equipment_zone_indices.get(i).copied().flatten() else {
                continue;
            };
            if zidx >= n_zones {
                continue;
            }
            let Some(m) = mode else { continue };
            match m {
                OperatingMode::Heating
                | OperatingMode::HeatingHP
                | OperatingMode::HeatingER
                | OperatingMode::HeatingHPAndER => zone_has_heating[zidx] = true,
                OperatingMode::Cooling => zone_has_cooling[zidx] = true,
                _ => {}
            }
        }
        for (zidx, hc) in simultaneous_hc_steps.iter_mut().enumerate() {
            if zone_has_heating[zidx] && zone_has_cooling[zidx] {
                *hc += 1;
            }
        }
    }
}

#[cfg(feature = "observe")]
fn zone_name_for(id: ZoneId, zone_names: &[String]) -> String {
    zone_names
        .get(id.0.saturating_sub(1) as usize)
        .cloned()
        .unwrap_or_else(|| format!("Zone({})", id.0))
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[cfg(feature = "observe")]
    use super::*;
    #[cfg(feature = "observe")]
    use hares_types::ZoneState;

    /// `record_from_state` moves its scratch out for the step and back: the
    /// same buffers (same allocation, same capacity) come back holding the
    /// step's extracted zone temperatures, and the counters see those values.
    #[cfg(feature = "observe")]
    #[test]
    fn record_from_state_returns_the_same_scratch_buffers_with_the_steps_values() {
        // One equipment slot so the equipment scratch vectors own an
        // allocation too (the step itself passes no equipment).
        let mut acc = DiagnosticAccumulator::new(
            &[ZoneId(1), ZoneId(2)],
            &[true, true],
            &["Heater".to_owned()],
            &[Some(ZoneId(1))],
        );
        let buffers = |acc: &DiagnosticAccumulator| {
            let s = &acc.scratch;
            [
                (s.zone_temps.as_ptr() as usize, s.zone_temps.capacity()),
                (s.heating_sps.as_ptr() as usize, s.heating_sps.capacity()),
                (s.cooling_sps.as_ptr() as usize, s.cooling_sps.capacity()),
                (
                    s.equipment_modes.as_ptr() as usize,
                    s.equipment_modes.capacity(),
                ),
                (
                    s.equipment_zone_indices.as_ptr() as usize,
                    s.equipment_zone_indices.capacity(),
                ),
            ]
        };
        let before = buffers(&acc);

        for temps in [[-1.0, 5.0], [-2.0, -3.0]] {
            let zones: Vec<ZoneState> = [ZoneId(1), ZoneId(2)]
                .into_iter()
                .zip(temps)
                .map(|(id, t)| ZoneState::new(id, t, 0.008, 100.0))
                .collect();
            acc.record_from_state(&zones, &[]);
            assert_eq!(
                buffers(&acc),
                before,
                "the scratch buffers must be moved back, not replaced"
            );
            assert_eq!(acc.scratch.zone_temps, temps);
        }

        assert_eq!(acc.total_steps, 2);
        let freezing: Vec<u64> = acc.zone_counters.iter().map(|z| z.freezing_steps).collect();
        assert_eq!(freezing, [2, 1]);
    }
}
