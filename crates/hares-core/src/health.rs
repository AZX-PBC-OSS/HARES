//! Run-level health totals for a dwelling.

#[cfg(feature = "profiling")]
use std::sync::Arc;
#[cfg(feature = "profiling")]
use std::time::Duration as StdDuration;

use serde::{Deserialize, Serialize};

use hares_types::{ThermalAccumulator, ZoneId};

use hares_physics::constants::SECONDS_PER_HOUR;

/// The first policy crossing one counter's change sites recorded: the
/// pieces a `HealthPolicyViolation` carries, with the step index the
/// dwelling stamps on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HealthCrossing {
    /// The equipment or actor the crossing's event came from.
    pub source: String,
    /// The run-total count at the crossing.
    pub count: u64,
    /// The limit the count crossed.
    pub limit: u64,
}

/// One dispatch pass's rejected-control-signal counting under the run's
/// health policy. Every rejection site records through [`Self::record`],
/// which counts and, when the run-total count (the baseline the gate was
/// built with plus the pass's rejections) crosses the policy's limit for
/// rejected signals, remembers the first crossing with its source. The
/// dwelling lands the pass's count on the run total and turns a recorded
/// crossing into the latched `HealthPolicyViolation` that ends the run.
#[derive(Debug)]
pub(crate) struct RejectionGate {
    baseline: u64,
    limit: Option<u64>,
    count: u64,
    crossing: Option<HealthCrossing>,
}

impl RejectionGate {
    /// A gate over the run total `baseline`, with the policy limit for
    /// rejected control signals (`None` when the policy sets none).
    pub(crate) fn new(baseline: u64, limit: Option<u64>) -> Self {
        Self {
            baseline,
            limit,
            count: 0,
            crossing: None,
        }
    }

    /// Counts one rejection from `source` and evaluates the policy on the
    /// new run-total count: only the first crossing is remembered, so the
    /// violation names the rejection that crossed.
    pub fn record(&mut self, source: &str) {
        self.count = self.count.saturating_add(1);
        if let Some(limit) = self.limit {
            let count = self.baseline + self.count;
            if count > limit && self.crossing.is_none() {
                self.crossing = Some(HealthCrossing {
                    source: source.to_owned(),
                    count,
                    limit,
                });
            }
        }
    }

    /// The pass's rejections and the first crossing it recorded, if any.
    pub(crate) fn into_parts(self) -> (u64, Option<HealthCrossing>) {
        (self.count, self.crossing)
    }
}

/// Run-total health counters for one dwelling, recorded unconditionally in
/// every build profile and never reset per step: a finished run always
/// reports the degradation it hit without requiring a special build or log
/// capture. Per-step observe counters do not exist; the run totals replace
/// them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunHealth {
    /// Equipment steps whose port contributions were rolled back after the
    /// step failed validation.
    pub port_rollbacks: u64,
    /// Control signals rejected by equipment (capability gate, numeric
    /// bounds) or routed at a target that does not exist.
    pub rejected_control_signals: u64,
    /// Equipment actions clamped to a safe bound instead of the requested
    /// value. Recorded via [`crate::dwelling::Dwelling::record_clamped_actions`].
    pub clamped_actions: u64,
    /// Total biquadratic curve-index clamps across all HVAC equipment over
    /// the run: an out-of-bounds curve index was evaluated and clamped to
    /// the last same-type curve.
    pub curve_index_clamps: u64,
    /// Fluid-payload absences after the fluid domain's first publish: a
    /// boiler stepped on its configured return temperature because the
    /// fluid domain published no payload that step. The first step's
    /// absence (before any publish) stays silent and is not counted.
    pub fluid_payload_absences: u64,
    /// Actors evicted because a roster change left one of their targets
    /// with no equipment to act on; each is also a dwelling warning.
    pub evicted_actors: u64,
    /// Runtime steps whose resolved thermostat setpoints sat below the
    /// required deadband: a constant override against the surviving
    /// schedule tier, the collision the init's static check cannot see.
    /// The run's first collision also raises a run warning.
    pub cross_tier_deadband_events: u64,
    /// Energy the failed equipment's committed internal state kept from a
    /// rolled-back step [Wh, signed]: the state's energy delta across the
    /// failed step (the reading of `Equipment::stored_energy_j` before
    /// the step and after the rollback). The rollback restores the port
    /// contributions but keeps the committed internal state, so this is
    /// energy the step's accounts no longer explain; the run's first
    /// nonzero event also raises a run warning naming the equipment and
    /// the energy.
    pub discarded_state_energy_wh: f64,
    /// Outcome of the warm-up convergence loop (Disabled until warm-up runs).
    pub warmup: WarmupOutcome,
    /// The run's energy-closure report: the integrated thermal balance the
    /// per-step `thermal_balance` invariant bounds per step, with the
    /// delivered end-uses the equipment ports metered. Zeroed until the
    /// first production step; warm-up steps are not part of the run's
    /// account.
    pub energy_closure: EnergyClosureReport,
}

impl Default for RunHealth {
    fn default() -> Self {
        Self {
            port_rollbacks: 0,
            rejected_control_signals: 0,
            clamped_actions: 0,
            curve_index_clamps: 0,
            fluid_payload_absences: 0,
            evicted_actors: 0,
            cross_tier_deadband_events: 0,
            discarded_state_energy_wh: 0.0,
            warmup: WarmupOutcome::Disabled,
            energy_closure: EnergyClosureReport::default(),
        }
    }
}

/// One registered actor's run-total scheduler-entry time, behind the
/// `profiling` feature: `total` is the actor's share of the run's `actors`
/// phase (the interest filter, `decide()` and the health check, measured by
/// the same phase-clock readings that accumulate the phase), `calls` the
/// number of `decide()` invocations the interest filter made.
#[cfg(feature = "profiling")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorTiming {
    pub name: Arc<str>,
    pub total: StdDuration,
    pub calls: u64,
}

/// Run-total per-actor timing store behind the `profiling` feature: one slot
/// per registered actor, in registration order, accumulated in place across
/// the run's steps and never reset per step (like [`RunHealth`]). Opaque by
/// contract (`plan.md` §2.9): the slots are private, `record` mutates them,
/// and the profiling summary reads them through [`Self::timings`].
#[cfg(feature = "profiling")]
#[derive(Debug, Default)]
pub struct ActorTimings {
    slots: Vec<ActorTiming>,
}

#[cfg(feature = "profiling")]
impl ActorTimings {
    /// One zeroed slot per name, in iteration order. Runs at actor
    /// registration (off the hot path), so the allocation is fine.
    pub fn new(names: impl IntoIterator<Item = Arc<str>>) -> Self {
        Self {
            slots: names
                .into_iter()
                .map(|name| ActorTiming {
                    name,
                    total: StdDuration::ZERO,
                    calls: 0,
                })
                .collect(),
        }
    }

    /// Adds `elapsed` to the slot's total in place and, when `called`,
    /// counts one `decide()` invocation. The accumulation allocates nothing:
    /// the scheduler calls this once per plan entry per step. An
    /// out-of-range slot has no actor (the actor list changed without the
    /// store being rebuilt, which the registration paths prevent) and is
    /// ignored in release builds.
    pub fn record(&mut self, slot: usize, elapsed: StdDuration, called: bool) {
        debug_assert!(
            slot < self.slots.len(),
            "actor timing slot {slot} out of range ({} slots)",
            self.slots.len()
        );
        if let Some(entry) = self.slots.get_mut(slot) {
            entry.total += elapsed;
            if called {
                entry.calls += 1;
            }
        }
    }

    /// The run's per-actor totals, one entry per registered actor in
    /// registration order.
    pub fn timings(&self) -> &[ActorTiming] {
        &self.slots
    }
}

/// One zone's run-total share of the energy-closure report [kWh].
///
/// The terms a zone's own wiring addresses: the end-use energy the
/// equipment ports delivered into the zone (sensible, radiant and
/// short-wave; latent is the humidity domain's, never part of the thermal
/// account) and the zone air node's own stored change. Wall and mass nodes
/// serve every zone through the shared A-matrix, so no zone row carries a
/// conduction term or a residual of its own: the closure identity and its
/// residual are the system row's.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ZoneClosureRow {
    /// The zone the row accounts.
    pub zone: ZoneId,
    /// Equipment-port delivery into the zone over the run [Wh].
    pub delivered_end_use_wh: f64,
    /// The zone air node's stored-energy change over the run [Wh],
    /// positive when the air stored heat. Excludes wall and mass nodes.
    pub zone_air_stored_wh: f64,
}

/// One closure row's integrated terms [Wh]: the gains side and the sinks
/// side of the run's thermal first law, the gross flux the per-step
/// relative tolerance scales with, and the integrated per-step residual.
/// The report's statement in kilowatt-hours is these Wh totals divided by
/// 1000; the unit lives in the docs, never in the identifier (the SI
/// naming rule).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ClosureRow {
    /// External gains integrated over the run [Wh]: the thermal solver's
    /// `B_d·u` external term (every joule entering the RC network from
    /// outside it: end-use delivery, passive gains, ambient- and
    /// ground-driven boundary fluxes), plus the affine coupling term the
    /// coupled steps carry.
    pub external_gains_wh: f64,
    /// Stored-energy change integrated over the run [Wh]:
    /// `Σ C_i·(T_next_i − T_prev_i)/dt` over every thermal node.
    pub stored_delta_wh: f64,
    /// Envelope conduction integrated over the run [Wh]: the thermal
    /// solver's conduction term, positive when heat leaves the network.
    pub envelope_conduction_wh: f64,
    /// Equipment-port delivery integrated over the run [Wh] (sensible,
    /// radiant and short-wave; latent is the humidity domain's). An
    /// attribution within the external gains, not a fourth balance term:
    /// the ports' delivery arrives in the network through the same `B_d·u`
    /// columns `external_gains_wh` integrates.
    pub delivered_end_use_wh: f64,
    /// Gross flux integrated over the run [Wh]: `Σ|q_gains|` per step, the
    /// quantity the per-step relative tolerance scales with. The residual's
    /// ppm denominator.
    pub gross_wh: f64,
    /// The integrated per-step residual [Wh]: `Σ (Σq_gains − ΔE_stored −
    /// q_loss)·dt`. Each step's residual is bounded by the per-step
    /// tolerance `max(1 W, 1e-6·gross)`, which a step can carry silently;
    /// integrated over a year that tolerance admits about 4.4 kWh (4400 Wh)
    /// of silent bias. This row states what actually accumulated.
    pub residual_wh: f64,
}

impl ClosureRow {
    /// Integrates one step's balance into the row.
    ///
    /// `q_gains` is the thermal solver's external-gain terms [W] (external,
    /// then the affine coupling term when the step is coupled),
    /// `delta_e_storage_w` the stored rate over all nodes [W], and
    /// `q_loss_w` the conduction term [W] (positive when heat leaves).
    /// `dt_s` is the step's length. The delivered end-uses are not a term
    /// of the identity (an attribution within the external gains, and a
    /// per-zone one: see [`ZoneClosureRow`]) and accumulate outside this
    /// method. The residual is accumulated per step, not recovered from the
    /// integrated columns: the per-step residual is the quantity the
    /// per-step tolerance bounds, and its integral is the silent bias this
    /// report makes visible.
    pub fn record_step(
        &mut self,
        q_gains: &[f64],
        delta_e_storage_w: f64,
        q_loss_w: f64,
        dt_s: f64,
    ) {
        let external_w: f64 = q_gains.iter().sum();
        let gross_w: f64 = q_gains.iter().map(|q| q.abs()).sum();
        let hours = dt_s / SECONDS_PER_HOUR;
        self.external_gains_wh += external_w * hours;
        self.stored_delta_wh += delta_e_storage_w * hours;
        self.envelope_conduction_wh += q_loss_w * hours;
        self.gross_wh += gross_w * hours;
        self.residual_wh += (external_w - delta_e_storage_w - q_loss_w) * hours;
    }

    /// The row's residual in parts per million of the gross flux. 0.0 when
    /// the run carried no gains (nothing stepped or the terms were never
    /// seen): a ppm of nothing is defined as nothing.
    #[must_use]
    pub fn residual_ppm_of_gross(&self) -> f64 {
        if self.gross_wh == 0.0 {
            0.0
        } else {
            self.residual_wh / self.gross_wh * 1e6
        }
    }
}

/// The run's energy-closure report: the thermal first law the per-step
/// `thermal_balance` invariant enforces, integrated over the run's
/// production steps and stated in energy instead of watts (Wh totals; the
/// kilowatt-hour form divides by 1000). Zeroed until the
/// first production step; warm-up steps are the convergence replay's, not
/// the run's account. The report is telemetry, never a health event: a
/// residual inside the per-step tolerance is tolerated by construction, and
/// this report exists to make its accumulation visible, not to fail the run
/// a second gate would have failed already.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EnergyClosureReport {
    /// The full-system closure row: the identity the per-step invariant
    /// checks, over every RC node.
    pub system: ClosureRow,
    /// Per-zone rows, in [`ZoneId`] order: the terms the zones' own wiring
    /// addresses. A zone with no thermal port and no air node holds no row.
    pub zones: Vec<ZoneClosureRow>,
}

impl EnergyClosureReport {
    /// Integrates one production step's balance into the report.
    ///
    /// `thermal_ports` is the step's thermal-port accumulators (the
    /// delivery the ports metered, per zone), `zone_air_stored_w` the
    /// solver's per-zone air-node stored rates [W], and the remaining
    /// scalars the full-system terms the thermal solver's
    /// `thermal_balance_terms` exposed. Allocates nothing in steady state:
    /// the zone rows' storage is sized once at the first step each zone
    /// appears.
    pub fn record_step(
        &mut self,
        q_gains: &[f64],
        delta_e_storage_w: f64,
        q_loss_w: f64,
        thermal_ports: &[ThermalAccumulator],
        zone_air_stored_w: &std::collections::HashMap<ZoneId, f64>,
        dt_s: f64,
    ) {
        let energy_wh = |watts: f64| watts * dt_s / SECONDS_PER_HOUR;
        self.system
            .record_step(q_gains, delta_e_storage_w, q_loss_w, dt_s);
        for port in thermal_ports {
            let delivered_w = port.sensible_gain_w + port.radiant_gain_w + port.shortwave_gain_w;
            self.system.delivered_end_use_wh += energy_wh(delivered_w);
            let pos = self.zones.partition_point(|row| row.zone < port.zone);
            let row = if self.zones.get(pos).is_some_and(|row| row.zone == port.zone) {
                &mut self.zones[pos]
            } else {
                // First sight of this zone: its row joins in ZoneId order,
                // the report's stored order and the reason the rows are
                // deterministic across runs.
                self.zones.insert(
                    pos,
                    ZoneClosureRow {
                        zone: port.zone,
                        delivered_end_use_wh: 0.0,
                        zone_air_stored_wh: 0.0,
                    },
                );
                &mut self.zones[pos]
            };
            row.delivered_end_use_wh += energy_wh(delivered_w);
        }
        for (&zone, &stored_w) in zone_air_stored_w {
            let stored_wh = energy_wh(stored_w);
            let pos = self.zones.partition_point(|row| row.zone < zone);
            if self.zones.get(pos).is_some_and(|row| row.zone == zone) {
                self.zones[pos].zone_air_stored_wh += stored_wh;
            } else {
                self.zones.insert(
                    pos,
                    ZoneClosureRow {
                        zone,
                        delivered_end_use_wh: 0.0,
                        zone_air_stored_wh: stored_wh,
                    },
                );
            }
        }
    }

    /// The system row's residual in parts per million of the gross flux.
    #[must_use]
    pub fn residual_ppm_of_gross(&self) -> f64 {
        self.system.residual_ppm_of_gross()
    }
}

/// How the warm-up convergence loop ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WarmupOutcome {
    /// Warm-up never ran (the run's warm-up is `Warmup::Disabled`, or a
    /// degenerate zero-step day).
    Disabled,
    /// Warm-up ran to a stop.
    Ran {
        /// Warm-up days simulated (iterations of the 24-hour replay).
        days_run: u32,
        /// Whether every zone's four EnergyPlus checks (daily maximum and
        /// minimum temperature, daily peak heating and cooling delivery)
        /// passed on a day at or after the minimum day count, before the
        /// maximum day count was reached.
        converged: bool,
        /// Day-over-day residuals of the final warm-up day against the day
        /// before it; `None` when only one warm-up day ran (no
        /// predecessor to diff against).
        residuals: Option<WarmupResiduals>,
    },
}

/// Day-over-day residuals of the final warm-up day, by the EnergyPlus
/// warm-up convergence criteria
/// (`vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc:3309-3430`).
///
/// The recorded row is the worst zone's: the zone with the largest
/// day-over-day daily-maximum temperature residual, with its own daily-min
/// temperature residual and its own daily-peak HVAC heating and cooling
/// relative changes beside it. The comparison checks every zone's four
/// residuals (all zones, unconditioned included, as EnergyPlus does); a
/// converged run's recorded row can still name a zone whose own residuals
/// were not the binding ones, and the non-convergence warning names every
/// zone failing a check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WarmupResiduals {
    /// Zone with the largest day-over-day daily-maximum temperature
    /// residual; the row's other residuals are this zone's own.
    pub worst_zone: ZoneId,
    /// |Δ daily max temperature| of [`Self::worst_zone`] [°C].
    pub max_temperature_c: f64,
    /// |Δ daily min temperature| of [`Self::worst_zone`] [°C].
    pub min_temperature_c: f64,
    /// Relative change of [`Self::worst_zone`]'s own daily peak heating
    /// delivered (fraction, EnergyPlus's 100 W floor and 1e-4 W
    /// pass-through applied).
    pub heating_load: f64,
    /// Relative change of [`Self::worst_zone`]'s own daily peak cooling
    /// delivered (fraction, EnergyPlus's 100 W floor and 1e-4 W
    /// pass-through applied).
    pub cooling_load: f64,
}

#[cfg(test)]
mod tests {
    use super::{ClosureRow, EnergyClosureReport, RunHealth, WarmupOutcome, WarmupResiduals};
    use hares_types::{ThermalAccumulator, ZoneId};
    #[test]
    fn run_health_defaults_to_disabled_warmup_and_zero_counters() {
        let health = RunHealth::default();
        assert_eq!(health.port_rollbacks, 0);
        assert_eq!(health.rejected_control_signals, 0);
        assert_eq!(health.clamped_actions, 0);
        assert_eq!(health.curve_index_clamps, 0);
        assert_eq!(health.fluid_payload_absences, 0);
        assert_eq!(health.evicted_actors, 0);
        assert_eq!(health.cross_tier_deadband_events, 0);
        assert_eq!(health.discarded_state_energy_wh, 0.0);
        assert_eq!(health.warmup, WarmupOutcome::Disabled);
        assert_eq!(health.energy_closure, EnergyClosureReport::default());
        assert_eq!(health.energy_closure.residual_ppm_of_gross(), 0.0);
    }

    #[test]
    fn run_health_round_trips_through_json() {
        let health = RunHealth {
            port_rollbacks: 1,
            rejected_control_signals: 2,
            clamped_actions: 3,
            curve_index_clamps: 4,
            fluid_payload_absences: 6,
            evicted_actors: 5,
            cross_tier_deadband_events: 7,
            discarded_state_energy_wh: 8.5,
            warmup: WarmupOutcome::Ran {
                days_run: 3,
                converged: false,
                residuals: Some(WarmupResiduals {
                    worst_zone: ZoneId(2),
                    max_temperature_c: 0.7,
                    min_temperature_c: 0.2,
                    heating_load: 0.05,
                    cooling_load: 0.0,
                }),
            },
            energy_closure: EnergyClosureReport {
                system: ClosureRow {
                    external_gains_wh: 10000.0,
                    stored_delta_wh: 4000.0,
                    envelope_conduction_wh: 6000.0,
                    delivered_end_use_wh: 9000.0,
                    gross_wh: 12000.0,
                    residual_wh: 1e-9,
                },
                zones: vec![super::ZoneClosureRow {
                    zone: ZoneId(1),
                    delivered_end_use_wh: 9000.0,
                    zone_air_stored_wh: 500.0,
                }],
            },
        };
        let json = serde_json::to_string(&health).expect("serialize RunHealth");
        let decoded: RunHealth = serde_json::from_str(&json).expect("deserialize RunHealth");
        assert_eq!(decoded, health);
    }

    /// The closure row integrates a steady balance exactly: 1000 W gains,
    /// 200 W stored, 800 W conduction, 3600 s. The identity closes per step,
    /// so the residual integral reads zero to the accumulation's own float
    /// noise, and the terms land at their hand-computed watt-hours.
    #[test]
    fn closure_row_integrates_a_steady_balance_to_zero_residual() {
        let mut row = ClosureRow::default();
        // 1000 W x 1 h = 1000 Wh exactly; 200 W x 1 h = 200 Wh; 800 Wh.
        row.record_step(&[1000.0], 200.0, 800.0, 3600.0);
        assert_eq!(row.external_gains_wh, 1000.0);
        assert_eq!(row.stored_delta_wh, 200.0);
        assert_eq!(row.envelope_conduction_wh, 800.0);
        assert_eq!(row.gross_wh, 1000.0);
        assert_eq!(row.residual_wh, 0.0);
        assert_eq!(row.residual_ppm_of_gross(), 0.0);
    }

    /// A 100 W loss the gains side never saw surfaces in the residual: 100 W
    /// for 3600 s is exactly 100 Wh, and the ppm denominator scales it
    /// against the row's own gross flux.
    #[test]
    fn closure_row_names_an_injected_uncounted_loss() {
        let mut row = ClosureRow::default();
        row.record_step(&[1000.0], 200.0, 800.0, 3600.0);
        row.record_step(&[1000.0], 200.0, 900.0, 3600.0);
        // Step 1 closes (residual 0); step 2 carries the 100 W uncounted loss:
        // 100 W x 1 h = 100 Wh integrated.
        assert!((row.residual_wh + 100.0).abs() < 1e-9);
        // 100 Wh against 2000 Wh of gross flux = 50,000 ppm.
        assert!((row.residual_ppm_of_gross() + 50_000.0).abs() < 1e-6);
    }

    /// The report's zone rows accumulate the ports' delivery per zone and the
    /// solver's per-zone air-node storage, in ZoneId order regardless of the
    /// iteration order of the maps feeding them, and the system row's
    /// delivered total is the ports' total.
    #[test]
    fn closure_report_accumulates_zone_rows_in_zone_order() {
        let port = |zone: ZoneId, sensible_w: f64| ThermalAccumulator {
            zone,
            sensible_gain_w: sensible_w,
            radiant_gain_w: 0.0,
            latent_gain_w: 0.0,
            shortwave_gain_w: 0.0,
            sensible_by_category: [0.0; hares_types::ports::THERMAL_CATEGORY_COUNT],
            radiant_by_category: [0.0; hares_types::ports::THERMAL_CATEGORY_COUNT],
            latent_by_category: [0.0; hares_types::ports::THERMAL_CATEGORY_COUNT],
        };
        let mut report = EnergyClosureReport::default();
        // 100 W into zone 2, 50 W into zone 1, one 3600 s step: 100 and
        // 50 Wh delivered.
        report.record_step(
            &[150.0],
            150.0,
            0.0,
            &[port(ZoneId(2), 100.0), port(ZoneId(1), 50.0)],
            &[(ZoneId(2), 90.0), (ZoneId(1), 60.0)].into_iter().collect(),
            3600.0,
        );
        // The zones Vec is in ZoneId order, not feed order.
        assert_eq!(
            report.zones.iter().map(|row| row.zone).collect::<Vec<_>>(),
            vec![ZoneId(1), ZoneId(2)]
        );
        assert_eq!(report.zones[0].delivered_end_use_wh, 50.0);
        assert!((report.zones[0].zone_air_stored_wh - 60.0).abs() < 1e-9);
        assert_eq!(report.zones[1].delivered_end_use_wh, 100.0);
        assert!((report.zones[1].zone_air_stored_wh - 90.0).abs() < 1e-9);
        // The system row: 150 Wh delivered, the balance's own integration.
        assert!((report.system.delivered_end_use_wh - 150.0).abs() < 1e-9);
        assert!((report.system.residual_wh).abs() < 1e-9);
    }
}
