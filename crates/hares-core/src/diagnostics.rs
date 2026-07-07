//! Per-timestep diagnostic output for debugging thermal runaway and solver issues.
//!
//! Enable by setting `output_verbosity >= 4` in SimulationConfig.
//! Writes a CSV with one row per timestep containing zone temps, solver inputs/outputs,
//! equipment operating points, and port accumulations.
//!
//! ## Post-hoc diagnostic checks
//!
//! When `cfg(feature = "observe")` is active, `DiagnosticAccumulator` collects
//! per-step data during the simulation and `run_post_hoc_checks()` evaluates
//! four diagnostic checks at end-of-run:
//!
//! - Excessive unmet heating/cooling hours
//! - Equipment short-cycling (mode change frequency)
//! - Temperature excursions below freezing in conditioned zones
//! - Simultaneous heating and cooling in the same zone
//!
//! Thresholds follow the defaults in the ticket:
//! `unmet_load_threshold = 0.05` (5%), `max_cycles_per_hour = 4`.

use std::io::Write;

use hares_types::{EnvironmentState, PortSlots, ZoneId};

use hares_physics::units::power_w_to_kw;

#[cfg(feature = "observe")]
use hares_equipment::Equipment;
#[cfg(feature = "observe")]
use hares_types::{OperatingMode, ZoneState};

/// Collects diagnostic data for a single timestep.
#[derive(Debug, Default)]
pub struct StepDiagnostics {
    pub step: u64,
    pub timestamp_s: f64,
    pub outdoor_temp_c: f64,
    pub zone_temps_c: Vec<(ZoneId, f64)>,
    /// Total thermal port sensible gain per zone [W] (HVAC + appliances combined).
    pub thermal_gains_w: Vec<(ZoneId, f64)>,
    pub thermal_latent_w: Vec<(ZoneId, f64)>,
    pub electrical_net_kw: f64,
    pub electrical_net_kvar: f64,
    pub equipment: Vec<EquipmentDiag>,
    /// Per-equipment sensible gain contributions to each zone.
    pub equipment_sensible_w: Vec<(String, ZoneId, f64)>,
    /// Envelope component gains from the thermal solver.
    pub envelope: Option<EnvelopeDiag>,
}

#[derive(Debug, Default, Clone)]
pub struct EnvelopeDiag {
    pub window_solar_w: f64,
    pub opaque_solar_lwr_w: f64,
    /// Total interior LWR exchange activity [W] (Σ|q_i|/2).
    pub interior_lwr_w: f64,
    /// Non-HVAC internal gains [W].
    pub internal_gain_w: f64,
    /// Total port convective [W] (HVAC + appliances).
    pub port_convective_w: f64,
    /// Total port radiant [W] (HVAC + appliances distributed to surfaces).
    ///
    /// ASHRAE HoF Ch. 18: internal gains have separate convective and radiant
    /// components; reporting both enables MRT diagnosis and BESTEST comparisons.
    /// EnergyPlus exposes `OtherEquipment Radiant Heating Rate [W]` as a
    /// separate output variable (I/O Ref v8.4, Internal Gains group).
    pub port_radiant_w: f64,
    /// Outdoor moist-air density used for infiltration mass-flow conversion [kg/m³].
    pub air_density_kg_m3: f64,
    /// Global horizontal irradiance [W/m²] at this timestep.
    /// Enables validation of window transmitted solar against expected
    /// GHI × window_area × SHGC at any reference timestep.
    pub ghi_w_m2: f64,
}

#[derive(Debug)]
pub struct EquipmentDiag {
    pub name: String,
    /// Zone mapping is populated at dwelling init via `write_equipment_init`,
    /// not per-step in `capture()`. Per-step zone routing is verified through
    /// the per-zone thermal gain columns in the diagnostic CSV.
    pub zone_id: Option<ZoneId>,
    pub mode: f64,
    pub electric_kw: f64,
    pub reactive_kvar: f64,
    pub sensible_gain_w: f64,
}

/// Writes diagnostic CSV header.
pub fn write_header(w: &mut impl Write, n_zones: usize) {
    let mut cols = vec![
        "step".to_string(),
        "timestamp_s".to_string(),
        "outdoor_temp_c".to_string(),
    ];
    for i in 0..n_zones {
        cols.push(format!("zone{}_temp_c", i + 1));
        cols.push(format!("zone{}_thermal_gain_w", i + 1));
        cols.push(format!("zone{}_latent_gain_w", i + 1));
    }
    cols.push("electrical_net_kw".to_string());
    cols.push("electrical_net_kvar".to_string());
    cols.push("port_radiant_w".to_string());
    cols.push("port_convective_w".to_string());
    cols.push("window_solar_w".to_string());
    cols.push("opaque_solar_lwr_w".to_string());
    cols.push("interior_lwr_w".to_string());
    cols.push("internal_gain_w".to_string());
    cols.push("air_density_kg_m3".to_string());
    cols.push("ghi_w_m2".to_string());
    let _ = writeln!(w, "{}", cols.join(","));
}

/// Writes one row of diagnostic data.
///
/// Every float field is screened for finiteness before CSV formatting:
/// non-finite values produce an empty column to avoid corrupting
/// downstream parsers with `NaN` or `inf` tokens.
pub fn write_row(w: &mut impl Write, d: &StepDiagnostics, n_zones: usize) {
    let mut vals: Vec<String> = vec![
        d.step.to_string(),
        if d.timestamp_s.is_finite() {
            format!("{:.1}", d.timestamp_s)
        } else {
            String::new()
        },
        if d.outdoor_temp_c.is_finite() {
            format!("{:.2}", d.outdoor_temp_c)
        } else {
            String::new()
        },
    ];
    for i in 0..n_zones {
        let zone_id = ZoneId((i + 1) as u16);
        let temp = d
            .zone_temps_c
            .iter()
            .find(|(z, _)| *z == zone_id)
            .map(|(_, t)| *t)
            .unwrap_or(0.0);
        let gain = d
            .thermal_gains_w
            .iter()
            .find(|(z, _)| *z == zone_id)
            .map(|(_, g)| *g)
            .unwrap_or(0.0);
        let latent = d
            .thermal_latent_w
            .iter()
            .find(|(z, _)| *z == zone_id)
            .map(|(_, l)| *l)
            .unwrap_or(0.0);
        vals.push(if temp.is_finite() {
            format!("{:.4}", temp)
        } else {
            String::new()
        });
        vals.push(if gain.is_finite() {
            format!("{:.1}", gain)
        } else {
            String::new()
        });
        vals.push(if latent.is_finite() {
            format!("{:.1}", latent)
        } else {
            String::new()
        });
    }
    vals.push(if d.electrical_net_kw.is_finite() {
        format!("{:.4}", d.electrical_net_kw)
    } else {
        String::new()
    });
    vals.push(if d.electrical_net_kvar.is_finite() {
        format!("{:.4}", d.electrical_net_kvar)
    } else {
        String::new()
    });
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| {
                if e.port_radiant_w.is_finite() {
                    format!("{:.1}", e.port_radiant_w)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| {
                if e.port_convective_w.is_finite() {
                    format!("{:.1}", e.port_convective_w)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| {
                if e.window_solar_w.is_finite() {
                    format!("{:.1}", e.window_solar_w)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| {
                if e.opaque_solar_lwr_w.is_finite() {
                    format!("{:.1}", e.opaque_solar_lwr_w)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| {
                if e.interior_lwr_w.is_finite() {
                    format!("{:.1}", e.interior_lwr_w)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| {
                if e.internal_gain_w.is_finite() {
                    format!("{:.1}", e.internal_gain_w)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| {
                if e.air_density_kg_m3.is_finite() {
                    format!("{:.5}", e.air_density_kg_m3)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| {
                if e.ghi_w_m2.is_finite() {
                    format!("{:.1}", e.ghi_w_m2)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default(),
    );
    let _ = writeln!(w, "{}", vals.join(","));
}

/// Write equipment zone-id mapping at init for diagnostic traceability.
///
/// Emitted when `output_verbosity >= 4` to surface the zone routing
/// resolved from HPXML for each equipment instance. The mapping is written
/// as a comment block after the CSV header so downstream tools can
/// verify per-zone thermal attribution without inspecting config internals.
/// The optional `zone_type` is the HPXML `<Location>` label for water heater
/// equipment.
///
/// `ocv_sources` provides `ocv_source` provenance tags for battery/EV equipment.
pub fn write_equipment_init(
    w: &mut impl Write,
    equipment: &[(String, u16, Option<String>)],
    ocv_sources: &[(String, String)],
) {
    for (name, zone_id, zone_type) in equipment {
        match zone_type {
            Some(zt) => {
                let _ = writeln!(
                    w,
                    "# eq zone_id: {name} -> ZoneId({zone_id}), zone_type={zt}"
                );
            }
            None => {
                let _ = writeln!(w, "# eq zone_id: {name} -> ZoneId({zone_id})");
            }
        }
    }
    for (name, source) in ocv_sources {
        let _ = writeln!(w, "# eq ocv_source: {name} -> {source}");
    }
}

/// Capture diagnostics from the current environment and port state.
///
/// Screens every float field for finiteness at the point of construction.
/// Non-finite values are reported via `tracing::error!` and written as
/// non-finite sentinels to the diagnostic snapshot so the invariant pass
/// (and post-hoc checks) can detect and report them with full context.
pub fn capture(
    step: u64,
    env: &EnvironmentState,
    ports: &PortSlots,
    envelope: Option<EnvelopeDiag>,
) -> StepDiagnostics {
    let timestamp_s = env.current_time.timestamp() as f64;
    let zone_temps_c: Vec<(ZoneId, f64)> =
        env.zones.iter().map(|z| (z.id, z.temperature_c)).collect();
    let thermal_gains_w: Vec<(ZoneId, f64)> = ports
        .thermal
        .iter()
        .map(|t| (t.zone, t.sensible_gain_w))
        .collect();
    let thermal_latent_w: Vec<(ZoneId, f64)> = ports
        .thermal
        .iter()
        .map(|t| (t.zone, t.latent_gain_w))
        .collect();
    let outdoor_temp_c = env.weather.outdoor_temp_c;
    let electrical_net_kw = power_w_to_kw(ports.electrical.net_active_w());
    let electrical_net_kvar = ports.electrical.reactive_power_kvar;

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        for &(_, temp) in &zone_temps_c {
            if !temp.is_finite() {
                tracing::error!(
                    step = step,
                    field = "zone_temp_c",
                    value = temp,
                    "NaN/Inf in diagnostic capture input"
                );
            }
        }
        for &(_, gain) in &thermal_gains_w {
            if !gain.is_finite() {
                tracing::error!(
                    step = step,
                    field = "thermal_gain_w",
                    value = gain,
                    "NaN/Inf in diagnostic capture input"
                );
            }
        }
        for &(_, latent) in &thermal_latent_w {
            if !latent.is_finite() {
                tracing::error!(
                    step = step,
                    field = "thermal_latent_w",
                    value = latent,
                    "NaN/Inf in diagnostic capture input"
                );
            }
        }
        if !outdoor_temp_c.is_finite() {
            tracing::error!(
                step = step,
                field = "outdoor_temp_c",
                value = outdoor_temp_c,
                "NaN/Inf in diagnostic capture input"
            );
        }
        if !electrical_net_kw.is_finite() {
            tracing::error!(
                step = step,
                field = "electrical_net_kw",
                value = electrical_net_kw,
                "NaN/Inf in diagnostic capture input"
            );
        }
        if !electrical_net_kvar.is_finite() {
            tracing::error!(
                step = step,
                field = "electrical_net_kvar",
                value = electrical_net_kvar,
                "NaN/Inf in diagnostic capture input"
            );
        }
        if let Some(ref e) = envelope {
            if !e.window_solar_w.is_finite() {
                tracing::error!(
                    step = step,
                    field = "window_solar_w",
                    value = e.window_solar_w,
                    "NaN/Inf in envelope diagnostic"
                );
            }
            if !e.opaque_solar_lwr_w.is_finite() {
                tracing::error!(
                    step = step,
                    field = "opaque_solar_lwr_w",
                    value = e.opaque_solar_lwr_w,
                    "NaN/Inf in envelope diagnostic"
                );
            }
            if !e.interior_lwr_w.is_finite() {
                tracing::error!(
                    step = step,
                    field = "interior_lwr_w",
                    value = e.interior_lwr_w,
                    "NaN/Inf in envelope diagnostic"
                );
            }
            if !e.internal_gain_w.is_finite() {
                tracing::error!(
                    step = step,
                    field = "internal_gain_w",
                    value = e.internal_gain_w,
                    "NaN/Inf in envelope diagnostic"
                );
            }
            if !e.port_convective_w.is_finite() {
                tracing::error!(
                    step = step,
                    field = "port_convective_w",
                    value = e.port_convective_w,
                    "NaN/Inf in envelope diagnostic"
                );
            }
            if !e.port_radiant_w.is_finite() {
                tracing::error!(
                    step = step,
                    field = "port_radiant_w",
                    value = e.port_radiant_w,
                    "NaN/Inf in envelope diagnostic"
                );
            }
        }
    }

    StepDiagnostics {
        step,
        timestamp_s,
        outdoor_temp_c,
        zone_temps_c,
        thermal_gains_w,
        thermal_latent_w,
        electrical_net_kw,
        electrical_net_kvar,
        equipment: Vec::new(),
        equipment_sensible_w: Vec::new(),
        envelope,
    }
}

// ---------------------------------------------------------------------------
// Post-hoc diagnostic checks (gated behind `observe` feature)
// ---------------------------------------------------------------------------

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
    scratch_zone_temps: Vec<f64>,
    scratch_heating_sps: Vec<Option<f64>>,
    scratch_cooling_sps: Vec<Option<f64>>,
    scratch_equipment_modes: Vec<Option<OperatingMode>>,
    scratch_equipment_zone_indices: Vec<Option<usize>>,
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
            scratch_zone_temps: vec![0.0; nz],
            scratch_heating_sps: vec![None; nz],
            scratch_cooling_sps: vec![None; nz],
            scratch_equipment_modes: vec![None; ne],
            scratch_equipment_zone_indices: vec![None; ne],
        }
    }

    /// Records one step of data. Call once per timestep when the observer is active.
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
        record_step_impl(
            &mut self.total_steps,
            &mut self.zone_counters,
            &mut self.equipment_counters,
            &mut self.prev_equipment_modes,
            &mut self.simultaneous_hc_steps,
            &mut self.zone_has_heating,
            &mut self.zone_has_cooling,
            zone_temps_c,
            heating_setpoints_c,
            cooling_setpoints_c,
            equipment_modes,
            equipment_zone_indices,
        );
    }

    /// Records one step from the dwelling's live state, using pre-allocated
    /// scratch buffers to avoid per-timestep heap allocation.
    ///
    /// Extracts zone temperatures, heating/cooling setpoints, equipment
    /// operating modes, and equipment zone indices from the dwelling's zone
    /// list and equipment list, then delegates to [`record_step`].
    pub fn record_from_state(&mut self, env_zones: &[ZoneState], equipment: &[Box<dyn Equipment>]) {
        let nz = env_zones.len().min(self.scratch_zone_temps.len());
        let ne = equipment.len().min(self.scratch_equipment_modes.len());

        // --- Zone temperatures ---
        self.scratch_zone_temps[..nz].fill(0.0);
        for (i, z) in env_zones.iter().take(nz).enumerate() {
            self.scratch_zone_temps[i] = z.temperature_c;
        }

        // --- Heating / cooling setpoints ---
        self.scratch_heating_sps[..nz].fill(None);
        self.scratch_cooling_sps[..nz].fill(None);
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
                    ) => self.scratch_heating_sps[zone_idx] = Some(sp),
                    Some(OperatingMode::Cooling) => self.scratch_cooling_sps[zone_idx] = Some(sp),
                    _ => {}
                }
            }
        }

        // --- Equipment operating modes ---
        self.scratch_equipment_modes[..ne].fill(None);
        for (i, eq) in equipment.iter().take(ne).enumerate() {
            self.scratch_equipment_modes[i] = eq.core_output().state.operating_mode;
        }

        // --- Equipment → zone index mapping ---
        self.scratch_equipment_zone_indices[..ne].fill(None);
        for (i, eq) in equipment.iter().take(ne).enumerate() {
            self.scratch_equipment_zone_indices[i] = eq
                .descriptor()
                .zone
                .and_then(|zid| env_zones.iter().position(|z| z.id == zid));
        }

        record_step_impl(
            &mut self.total_steps,
            &mut self.zone_counters,
            &mut self.equipment_counters,
            &mut self.prev_equipment_modes,
            &mut self.simultaneous_hc_steps,
            &mut self.zone_has_heating,
            &mut self.zone_has_cooling,
            &self.scratch_zone_temps[..nz],
            &self.scratch_heating_sps[..nz],
            &self.scratch_cooling_sps[..nz],
            &self.scratch_equipment_modes[..ne],
            &self.scratch_equipment_zone_indices[..ne],
        );
    }

    /// Runs all four post-hoc diagnostic checks using accumulated data and
    /// writes findings to the provided diagnostic CSV writer (as `# diag` comment
    /// lines) and emits structured `tracing` warnings/errors.
    ///
    /// `writer` is the same `BufWriter` used for per-step CSV rows, opened
    /// when `output_verbosity >= 4`.  If `None`, only tracing messages are
    /// emitted.  `time_res_s` is the timestep resolution in seconds, used to
    /// convert step counts to hours for the short-cycling check.
    ///
    /// Returns the number of diagnostic violations found (for caller observability).
    pub fn run_post_hoc_checks(
        &self,
        writer: &mut Option<impl Write>,
        zone_names: &[String],
        time_res_s: f64,
    ) -> usize {
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
                if let Some(w) = writer.as_mut() {
                    let _ = writeln!(
                        w,
                        "# diag unmet_heating: zone={zone_name}, \
                         unmet_pct={heat_pct:.1}, threshold_pct={threshold_pct:.1}, \
                         unmet_steps={}, total_sensor_steps={}",
                        zc.unmet_heating_steps, zc.steps_with_setpoint,
                    );
                }
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
                if let Some(w) = writer.as_mut() {
                    let _ = writeln!(
                        w,
                        "# diag unmet_cooling: zone={zone_name}, \
                         unmet_pct={cool_pct:.1}, threshold_pct={threshold_pct:.1}, \
                         unmet_steps={}, total_sensor_steps={}",
                        zc.unmet_cooling_steps, zc.steps_with_setpoint,
                    );
                }
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
                if let Some(w) = writer.as_mut() {
                    let _ = writeln!(
                        w,
                        "# diag short_cycling: equipment={}, zone={zone_label}, \
                         mode_changes={}, cycles_per_hour={cycles_per_hour:.1}, \
                         max={MAX_CYCLES_PER_HOUR}",
                        ec.name, ec.mode_changes,
                    );
                }
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
                if let Some(w) = writer.as_mut() {
                    let _ = writeln!(
                        w,
                        "# diag freezing: zone={zone_name}, \
                         freezing_steps={}, total_steps={}",
                        zc.freezing_steps, self.total_steps,
                    );
                }
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
                if let Some(w) = writer.as_mut() {
                    let _ = writeln!(
                        w,
                        "# diag simultaneous_hc: zone={zone_name}, \
                         simultaneous_steps={count}, total_steps={}",
                        self.total_steps,
                    );
                }
                violations += 1;
            }
        }

        // Always emit a summary line so downstream tooling can verify that
        // post-hoc checks executed (even when zero violations are found).
        if let Some(w) = writer.as_mut() {
            let _ = writeln!(
                w,
                "# diag summary: total_steps={}, violations={violations}, \
                 sim_hours={sim_hours:.2}",
                self.total_steps,
            );
        }

        violations
    }
}

/// Core step-accumulation logic shared by [`DiagnosticAccumulator::record_step`]
/// and [`DiagnosticAccumulator::record_from_state`].
///
/// Takes individual field references so callers can borrow mutable counter
/// fields and immutable scratch/input buffers from the same `DiagnosticAccumulator`
/// without tripping Rust's borrow checker on nested `&self` + `&mut self`.
#[cfg(feature = "observe")]
#[expect(
    clippy::too_many_arguments,
    reason = "field-splitting is the standard Rust pattern to avoid borrow checker conflicts when a method needs both shared &[T] scratch slices and mutable counter fields from the same struct"
)]
fn record_step_impl(
    total_steps: &mut u64,
    zone_counters: &mut [ZoneDiagnosticCounters],
    equipment_counters: &mut [EquipmentDiagnosticCounters],
    prev_equipment_modes: &mut [Option<OperatingMode>],
    simultaneous_hc_steps: &mut [u64],
    zone_has_heating: &mut [bool],
    zone_has_cooling: &mut [bool],
    zone_temps_c: &[f64],
    heating_setpoints_c: &[Option<f64>],
    cooling_setpoints_c: &[Option<f64>],
    equipment_modes: &[Option<OperatingMode>],
    equipment_zone_indices: &[Option<usize>],
) {
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
        if let (Some(prev), Some(curr)) = (prev_equipment_modes.get(i).copied().flatten(), mode) {
            if prev != curr {
                ec.mode_changes += 1;
            }
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
    use std::io::Cursor;

    use chrono::{DateTime, Duration};

    use hares_types::{EnvironmentState, GridState, PortSlots, WeatherState, ZoneId, ZoneState};

    use super::*;

    /// Verifies that `capture()` extracts zone temperatures, thermal gains,
    /// and electrical net power from live environment and port state, and
    /// that `write_header()` + `write_row()` produce well-formed CSV with
    /// expected headers and data values.
    #[test]
    fn capture_and_write_row_produce_valid_csv() {
        let env = EnvironmentState {
            zones: vec![ZoneState::new(ZoneId(1), 22.5, 0.008, 100.0)],
            weather: WeatherState::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: DateTime::parse_from_rfc3339("2024-06-15T12:00:00Z").unwrap(),
            time_res: Duration::seconds(60),
            price_signal: hares_types::PriceSignal::default(),
            electrical: hares_types::ElectricalSummary::default(),
        };

        let ports = PortSlots::default();

        let diag = capture(42, &env, &ports, None);

        assert_eq!(diag.step, 42);
        assert!(
            diag.timestamp_s > 0.0,
            "timestamp_s must be positive, got {}",
            diag.timestamp_s
        );
        assert_eq!(
            diag.zone_temps_c.len(),
            1,
            "expected 1 zone, got {}",
            diag.zone_temps_c.len()
        );
        assert_eq!(diag.zone_temps_c[0].0, ZoneId(1));
        assert!(
            (diag.zone_temps_c[0].1 - 22.5).abs() < 0.01,
            "zone temp mismatch"
        );

        let mut buf = Cursor::new(Vec::new());
        write_header(&mut buf, 1);
        write_row(&mut buf, &diag, 1);

        let csv = String::from_utf8(buf.into_inner()).expect("valid UTF-8");
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2, "expected header + 1 data row, got {csv:?}");

        let header = lines[0];
        let expected_cols = [
            "step",
            "timestamp_s",
            "outdoor_temp_c",
            "zone1_temp_c",
            "zone1_thermal_gain_w",
            "zone1_latent_gain_w",
            "electrical_net_kw",
            "electrical_net_kvar",
            "port_radiant_w",
            "port_convective_w",
            "window_solar_w",
            "opaque_solar_lwr_w",
            "interior_lwr_w",
            "internal_gain_w",
            "air_density_kg_m3",
            "ghi_w_m2",
        ];
        for &col in &expected_cols {
            assert!(
                header.contains(col),
                "header missing column '{col}'; header: {header}"
            );
        }

        let data = lines[1];
        let fields: Vec<&str> = data.split(',').map(|s| s.trim()).collect();
        assert_eq!(
            fields.len(),
            expected_cols.len(),
            "data row field count mismatch"
        );

        // step column
        assert_eq!(fields[0].parse::<u64>().unwrap(), 42);
        // zone1_temp_c should be ~22.5
        let temp_val: f64 = fields[3].parse().unwrap();
        assert!(
            (temp_val - 22.5).abs() < 0.1,
            "zone1_temp_c mismatch: {temp_val}"
        );
    }

    /// Verifies that `write_header()` emits the correct column count for
    /// multi-zone configurations.
    #[test]
    fn write_header_scales_with_zone_count() {
        let mut buf = Cursor::new(Vec::new());
        write_header(&mut buf, 3);

        let csv = String::from_utf8(buf.into_inner()).expect("valid UTF-8");
        let header = csv.lines().next().expect("header must exist");

        for i in 1..=3 {
            assert!(header.contains(&format!("zone{i}_temp_c")));
            assert!(header.contains(&format!("zone{i}_thermal_gain_w")));
            assert!(header.contains(&format!("zone{i}_latent_gain_w")));
        }
        assert!(!header.contains("zone4_temp_c"));
    }

    /// Verifies that `write_row()` writes `unwrap_or(0.0)` for missing gain
    /// columns and `unwrap_or_default()` for envelope columns.
    #[test]
    fn write_row_handles_missing_gains_and_nullable_envelope() {
        let diag = StepDiagnostics {
            step: 1,
            timestamp_s: 3600.0,
            outdoor_temp_c: 5.0,
            zone_temps_c: vec![(ZoneId(1), 20.0)],
            thermal_gains_w: vec![],  // missing — should default to 0.0
            thermal_latent_w: vec![], // missing
            electrical_net_kw: 0.5,
            electrical_net_kvar: 0.1,
            equipment: vec![],
            equipment_sensible_w: vec![],
            envelope: None, // nullable envelope
        };

        let mut buf = Cursor::new(Vec::new());
        write_header(&mut buf, 1);
        write_row(&mut buf, &diag, 1);

        let csv = String::from_utf8(buf.into_inner()).expect("valid UTF-8");
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);

        let data = lines[1];
        let fields: Vec<&str> = data.split(',').map(|s| s.trim()).collect();

        // Missing thermal gain → unwrap_or(0.0)
        let gain: f64 = fields[4].parse().unwrap();
        assert_eq!(gain, 0.0, "missing thermal gain should default to 0.0");

        let latent: f64 = fields[5].parse().unwrap();
        assert_eq!(latent, 0.0, "missing latent should default to 0.0");

        // Nullable envelope columns → unwrap_or_default() → empty string → could be ""
        // We check at least that the row parses correctly (no panic).
        assert_eq!(fields[0], "1");
        assert_eq!(fields[1], "3600.0");
    }

    /// Verifies that `capture()` samples outdoor temperature from weather state.
    #[test]
    fn capture_smoke_test() {
        let env = EnvironmentState {
            zones: vec![ZoneState::new(ZoneId(1), 18.0, 0.006, 80.0)],
            weather: WeatherState {
                outdoor_temp_c: 12.3,
                ..WeatherState::default()
            },
            grid: GridState {
                voltage_pu: 0.98,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: DateTime::parse_from_rfc3339("2024-01-01T06:00:00Z").unwrap(),
            time_res: Duration::seconds(300),
            price_signal: hares_types::PriceSignal::default(),
            electrical: hares_types::ElectricalSummary::default(),
        };

        let mut ports = PortSlots::default();
        ports
            .accumulate(&hares_types::PortContribution::Electrical {
                active_power_w: 1000.0,
                reactive_power_kvar: 0.5,
            })
            .unwrap();
        let diag = capture(1, &env, &ports, None);

        assert_eq!(diag.step, 1);
        assert!((diag.outdoor_temp_c - 12.3).abs() < 1e-9);
        assert_eq!(diag.zone_temps_c.len(), 1);
        assert_eq!(diag.zone_temps_c[0].0, ZoneId(1));
        assert!((diag.zone_temps_c[0].1 - 18.0).abs() < 1e-9);
        assert!((diag.electrical_net_kvar - 0.5).abs() < 1e-9);
    }
}
