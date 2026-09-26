//! EV Driver Actor -- behavioral proxy for EV charging decisions.
//!
//! Models a human driver's daily routine: departure, driving (multi-step
//! energy drain), arrival, plug-in, and charging strategy selection.
//! The actor dispatches `ControlSignal` variants (`EvPlugIn`, `EvDrive`,
//! `EvSetReadyBy`, `SOCTarget`) to the EV equipment via the control pipeline.
//!
//! Equipment is self-contained with its own BMS. The driver actor only
//! pushes external decisions -- it never mutates equipment state directly.
//! The driver's telemetry channels, however, report what is actually
//! happening at the vehicle: `soc` and `charge_kw` read the equipment's
//! committed output (previous step) whenever the equipment is observable,
//! so a caller watching only the driver's channels sees charging as it
//! occurs even under goal-based dispatches the equipment paces itself.
//!
//! ## Dispatch contract with the equipment
//!
//! Silence is never dispatched. Every plugged-in step asserts the resolved
//! decision: an idling strategy dispatches an explicit zero-power hold
//! (`PowerSetpoint{0}`), an active one dispatches its target and/or rate.
//! A plugged-in EV that receives no instruction charges toward its BMS
//! `ready_soc` default at rated power, so a silent idle step would silently
//! mean "charge to full" — the failure this module's composer explicitly
//! avoids. The equipment clears the hold when a fresh `SOCTarget`/
//! `EvSetReadyBy` arrives or on disconnect, so a hold never outlives the
//! idle window that produced it.
//!
//! ## SOC estimation model
//!
//! The actor maintains `estimated_soc` as its best guess of battery state.
//! This intentionally diverges from actual equipment SOC because the actor
//! does not observe CC-CV taper, thermal derating, or BMS charge termination.
//! All driver behavioral decisions -- plug-in, range anxiety, charging strategy
//! -- operate on `perceived_soc()` which returns `estimated_soc`. The ground-truth
//! `actual_soc()` reads `equipment_core` and exists for reconciliation,
//! observability, and diagnostics; the `soc` telemetry channel reports it
//! directly. The divergence is bounded and conservative:
//! the actor overestimates discharge and underestimates charge, causing it to
//! over-charge rather than strand the driver.

mod composer;
mod departure;
mod efficiency;
mod preference;
mod price;
mod soc_gate;
mod soc_target;
mod solar;
mod time_window;
mod v2g;
mod v2h;

use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Datelike, Timelike};
use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::telemetry_keys as tk;
use hares_types::{
    ChargingStrategy, ControlSignal, EnvironmentState, EquipmentId, EvConnectionState, HaresError,
    PlugInPolicy, ScheduleSource, Telemetry,
};
use rand::RngExt;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::Actor;

use self::composer::ChargingComposer;
use self::departure::DepartureDeadline;
use self::preference::{
    ChargingPreference, DecisionContext, minutes_until, needed_charge_hours_to_target,
};
use self::price::PriceOptimizer;
use self::soc_gate::SocGate;
use self::soc_target::SocTarget;
use self::solar::SolarTracking;
use self::time_window::TimeWindowPref;
use self::v2g::V2GExport;
use self::v2h::V2HDischarge;

/// A rolled daily driving event.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct DayEvent {
    departure_minute: u16,
    arrival_minute: u16,
    drive_kwh: f64,
}

/// State machine phase for the driver's day.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
enum DriverPhase {
    /// At home, plugged in (or waiting to plug in).
    HomePluggedIn,
    /// Currently driving (multi-step drain). Carries the departed day's
    /// plan: a trip whose steps cross midnight into a day that rolls
    /// non-driving must still complete on its own schedule — the day roll
    /// decides only *future* departures — so the trip's energy budget and
    /// home-arrival minute travel with the phase, not with `todays_event`
    /// (which the new day's roll may have replaced or cleared).
    Driving {
        remaining_kwh: f64,
        total_steps: u32,
        steps_done: u32,
        plan: DayEvent,
    },
    /// Away from home (parked, not driving), awaiting the departed trip's
    /// home-arrival minute — carried from the same plan (see `Driving`).
    Away { plan: DayEvent },
}

/// Hysteresis band (SOC fraction) for SocGate lower_threshold.
///
/// 0.05 (5 percentage points) is sufficient to suppress rapid charge/no-charge
/// cycling from typical BMS estimation noise, self-discharge rates, and
/// auxiliary-load fluctuations in residential EV use. The band is applied
/// symmetrically below the upper threshold for both LowSoc and QuickThenWait
/// strategies.
const SOC_GATE_DEFAULT_HYSTERESIS_BAND: f64 = 0.05;

/// Base charging efficiency (dimensionless) used for energy and
/// time-to-charge calculations. Temperature degradation is applied on top
/// (see `efficiency::temp_efficiency_multiplier`).
const CHARGING_EFFICIENCY: f64 = 0.9;

/// Build the preference stack for a given charging strategy.
fn build_preferences(
    strategy: &ChargingStrategy,
    max_charge_kw: f64,
    efficiency: f64,
    price_schedule: Option<Arc<[f64]>>,
    steps_per_day: usize,
) -> Vec<Box<dyn ChargingPreference>> {
    match strategy {
        ChargingStrategy::Immediate { target_soc } => {
            vec![Box::new(SocTarget {
                target_soc: *target_soc,
                charging_efficiency: efficiency,
            })]
        }
        ChargingStrategy::Nightly {
            off_peak_start_hour,
            off_peak_end_hour,
            target_soc,
        } => {
            vec![
                Box::new(TimeWindowPref::from_hours(
                    *off_peak_start_hour,
                    *off_peak_end_hour,
                )),
                Box::new(SocTarget {
                    target_soc: *target_soc,
                    charging_efficiency: efficiency,
                }),
            ]
        }
        ChargingStrategy::LowSoc {
            threshold,
            target_soc,
        } => {
            vec![Box::new(SocGate {
                upper_threshold: *threshold,
                lower_threshold: *threshold - SOC_GATE_DEFAULT_HYSTERESIS_BAND,
                target_soc: *target_soc,
                charging_efficiency: efficiency,
                charging_allowed: true,
            })]
        }
        ChargingStrategy::QuickThenWait { partial_soc } => {
            vec![Box::new(SocGate {
                upper_threshold: *partial_soc,
                lower_threshold: *partial_soc - SOC_GATE_DEFAULT_HYSTERESIS_BAND,
                target_soc: *partial_soc,
                charging_efficiency: efficiency,
                charging_allowed: true,
            })]
        }
        ChargingStrategy::PreDeparture {
            target_soc,
            departure_schedule,
        } => {
            vec![
                Box::new(DepartureDeadline {
                    schedule: departure_schedule.clone(),
                    target_soc: *target_soc,
                    efficiency,
                    buffer_hours: 0.0,
                }),
                Box::new(SocTarget {
                    target_soc: *target_soc,
                    charging_efficiency: efficiency,
                }),
            ]
        }
        ChargingStrategy::TouAware {
            target_soc,
            departure_schedule,
            charge_buffer_hours,
        } => {
            vec![
                Box::new(PriceOptimizer::new(
                    0.25,
                    0.75,
                    price_schedule,
                    steps_per_day,
                )),
                Box::new(DepartureDeadline {
                    schedule: departure_schedule.clone(),
                    target_soc: *target_soc,
                    efficiency,
                    buffer_hours: *charge_buffer_hours,
                }),
                Box::new(SocTarget {
                    target_soc: *target_soc,
                    charging_efficiency: efficiency,
                }),
            ]
        }
        ChargingStrategy::SolarSurplus {
            min_charge_rate_kw,
            departure_schedule,
        } => {
            vec![
                Box::new(SolarTracking {
                    min_charge_rate_kw: *min_charge_rate_kw,
                }),
                Box::new(DepartureDeadline {
                    schedule: departure_schedule.clone(),
                    target_soc: 1.0,
                    efficiency,
                    buffer_hours: 0.0,
                }),
            ]
        }
        ChargingStrategy::V2H {
            discharge_threshold_soc,
            min_soc,
        } => {
            vec![
                Box::new(SocTarget {
                    target_soc: 0.9,
                    charging_efficiency: efficiency,
                }),
                Box::new(V2HDischarge {
                    threshold_soc: *discharge_threshold_soc,
                    min_soc: *min_soc,
                    max_discharge_kw: max_charge_kw,
                }),
            ]
        }
        ChargingStrategy::V2G {
            min_soc,
            max_export_kw,
            price_threshold,
        } => {
            vec![
                Box::new(SocTarget {
                    target_soc: 1.0,
                    charging_efficiency: efficiency,
                }),
                Box::new(V2GExport {
                    min_soc: *min_soc,
                    max_export_kw: *max_export_kw,
                    price_threshold: *price_threshold,
                }),
            ]
        }
    }
}

/// Actor that models a human EV driver's daily behavior.
///
/// Rolls a stochastic daily event (departure time, arrival time, miles driven)
/// and dispatches the corresponding plug-in, driving, and charging signals
/// to the target EV equipment.
pub struct EvDriverActor {
    name: Arc<str>,
    dispatch_target: DispatchTarget,
    equipment_id: Option<EquipmentId>,

    // Behavioral config
    strategy: ChargingStrategy,
    plug_in_policy: PlugInPolicy,
    daily_drive_miles: ScheduleSource,
    departure_time: ScheduleSource,
    trip_duration: ScheduleSource,
    /// Direct arrival-time sampling. `None` means fall back to `departure + trip_duration`.
    arrival_time: Option<ScheduleSource>,
    event_day_ratio: f64,
    fuel_economy_kwh_per_mi: f64,
    capacity_kwh: f64,
    max_charge_kw: f64,
    average_speed_mph: f64,
    range_anxiety_miles: f64,
    away_charge_fraction: f64,
    away_charge_power_kw: f64,

    // Composer for per-step charging decisions
    composer: ChargingComposer,

    // Runtime state
    // NOTE: estimated_soc tracks the actor's best guess of EV SOC. It diverges
    // from actual equipment SOC because the actor doesn't observe CC-CV taper,
    // thermal derating, or BMS charge termination. The divergence is bounded
    // and conservative: the actor overestimates discharge and underestimates
    // charge, causing it to over-charge rather than strand the driver.
    rng: ChaCha8Rng,
    current_day_ordinal: i32,
    todays_event: Option<DayEvent>,
    phase: DriverPhase,
    estimated_soc: f64,
    time_res_minutes: f64,
    expected_daily_miles: f64,
    /// Deferred away-charge signals to emit on the next step after driving ends.
    needs_away_charge: bool,
    /// Cumulative count of trips cancelled because the pack could not cover
    /// them (the driver stayed home and charged). Published as the
    /// `drive_cancelled` telemetry channel so the shortfall is reported,
    /// never silently dropped mobility.
    drive_cancelled: u32,
    /// Actor telemetry: observable decision state for diagnostics.
    telemetry: Telemetry,
}

use efficiency::temp_efficiency_multiplier;

impl EvDriverActor {
    /// Creates a new EV driver actor.
    ///
    /// `rng` is required for deterministic behavior. All stochastic draws
    /// derive from this RNG. Callers using the dwelling RNG hierarchy should
    /// pass a pre-configured `ChaCha8Rng` from `derive_sub_rng` so the stream
    /// nonce is preserved.
    // Why: all parameters are independent behavioral inputs with no sensible defaults —
    // the actor's stochastic behavior depends on each being explicitly set by the caller.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        target: &str,
        strategy: ChargingStrategy,
        plug_in_policy: PlugInPolicy,
        daily_drive_miles: ScheduleSource,
        departure_time: ScheduleSource,
        trip_duration: ScheduleSource,
        arrival_time: Option<ScheduleSource>,
        event_day_ratio: f64,
        fuel_economy_kwh_per_mi: f64,
        capacity_kwh: f64,
        max_charge_kw: f64,
        average_speed_mph: f64,
        range_anxiety_miles: f64,
        away_charge_fraction: f64,
        away_charge_power_kw: f64,
        rng: ChaCha8Rng,
    ) -> Self {
        let prefs = build_preferences(&strategy, max_charge_kw, CHARGING_EFFICIENCY, None, 24);
        let composer = ChargingComposer::new(prefs, target);
        let expected_daily_miles = daily_drive_miles.mean();

        #[cfg(feature = "observe")]
        tracing::debug!(
            actor = name,
            distribution = ?daily_drive_miles,
            analytical_mean_mi = expected_daily_miles,
            "EV driver daily-miles distribution"
        );

        let mut telemetry = Telemetry::with_capacity(9);
        // Why: discharge_min_soc = 0.0 means "no discharge floor active" —
        // the field is populated per-step from min_soc on dispatched
        // PowerSetpoint signals. Only V2G/V2H strategies set a non-zero
        // floor. A discharge floor of 0% SOC (= allow full discharge) is
        // physically redundant (all strategies allow full discharge by
        // default), so 0.0 cannot collide with a meaningful constraint.
        telemetry.insert("soc", 1.0);
        telemetry.insert("phase", 0.0);
        telemetry.insert("charge_kw", 0.0);
        telemetry.insert("plugged_in", 1.0);
        telemetry.insert("soc_gate_charging_allowed", 1.0);
        telemetry.insert("needed_charge_hours", 0.0);
        telemetry.insert("discharge_min_soc", 0.0);
        // 1.0 while the driver's perceived SOC sits below the range-anxiety
        // band (tomorrow's trip plus the safety buffer): the driver is short
        // and will override their usual charging pattern once time to the
        // next departure runs short, whether or not the urgency gate has
        // fired yet on this step. Predictive state for evaluators: a managed
        // charging program can read which drivers its strategy is leaving
        // short before the out-of-pattern charge lands in the profile.
        telemetry.insert("range_anxiety_active", 0.0);
        telemetry.insert("drive_cancelled", 0.0);

        Self {
            name: Arc::from(name),
            dispatch_target: DispatchTarget::ByName(target.into()),
            equipment_id: None,
            strategy,
            plug_in_policy,
            daily_drive_miles,
            departure_time,
            trip_duration,
            arrival_time,
            event_day_ratio,
            fuel_economy_kwh_per_mi,
            capacity_kwh,
            max_charge_kw,
            average_speed_mph,
            range_anxiety_miles,
            away_charge_fraction,
            away_charge_power_kw,
            composer,
            rng,
            current_day_ordinal: -1,
            todays_event: None,
            phase: DriverPhase::HomePluggedIn,
            estimated_soc: 1.0,
            time_res_minutes: 1.0,
            expected_daily_miles,
            needs_away_charge: false,
            drive_cancelled: 0,
            telemetry,
        }
    }

    /// Set the price schedule for TOU-aware strategies, rebuilding the composer.
    pub fn with_price_schedule(mut self, schedule: Arc<[f64]>, steps_per_day: usize) -> Self {
        let target = self.target_name().to_owned();
        let prefs = build_preferences(
            &self.strategy,
            self.max_charge_kw,
            CHARGING_EFFICIENCY,
            Some(schedule),
            steps_per_day,
        );
        self.composer = ChargingComposer::new(prefs, &target);
        self
    }

    /// Returns the last charging action taken by the composer (for telemetry).
    pub fn last_action(&self) -> &str {
        self.composer.last_action()
    }

    /// Latest observed charger-mediated power at the vehicle (kW), from the
    /// equipment's committed output of the previous step. Positive =
    /// charging, negative = V2G/V2L export. Goal-based dispatches
    /// (`SOCTarget`, `EvSetReadyBy`) carry no rate — the equipment's BMS
    /// computes the real power — so this observation is the only faithful
    /// source for `charge_kw` under them.
    ///
    /// Face: while plugged in at home this is the equipment's grid-side
    /// electric flow (the same face as the equipment's own end-use power
    /// column). Away charging is deliberately excluded from that flow
    /// (off-site, no residential port contribution — see the EV equipment's
    /// `AwayPluggedIn` step), so the away phase reads the equipment's
    /// `away_charge_power_kw` telemetry instead.
    fn observed_charge_kw(&self, env: &EnvironmentState) -> Option<f64> {
        match self.phase {
            DriverPhase::HomePluggedIn => self
                .equipment_id
                .and_then(|id| env.equipment_core.get(&id))
                .and_then(|co| co.flows.electric_kw)
                .map(|p| p.signed_kw()),
            // allowed: away charge power is telemetry-only by design — while
            // AwayPluggedIn the equipment zeroes its residential
            // active_power_kw (the away charger is off-site from the
            // dwelling's electrical balance), so core_output's
            // electric_kw cannot carry it; equipment_telemetry is the only
            // channel that does, and it is name-keyed.
            DriverPhase::Away { .. } => env
                .equipment_telemetry
                .get(self.target_name())
                .and_then(|t| t.get(tk::AWAY_CHARGE_POWER_KW)),
            DriverPhase::Driving { .. } => None,
        }
    }

    fn populate_telemetry(
        &mut self,
        env: &EnvironmentState,
        before_out: usize,
        out: &[DispatchRequest],
    ) {
        // SOC channel: report the equipment's ground-truth SOC whenever the
        // equipment core is observable. `estimated_soc` is the driver's
        // behavioral belief — reconciled only at observation events
        // (arrival, day start) — and can sit far from truth across a
        // multi-hour charging span, which made this channel read as "the car
        // never charges". When the equipment is not registered the channel
        // degrades to the estimate (`resolve_equipment_id` has already warned
        // in that mode).
        let observed_soc = self.actual_soc(env);
        self.telemetry
            .set("soc", observed_soc.unwrap_or(self.estimated_soc));
        self.telemetry.set("phase", phase_as_f64(self.phase));
        self.telemetry.set(
            "plugged_in",
            if matches!(self.phase, DriverPhase::HomePluggedIn) {
                1.0
            } else {
                0.0
            },
        );

        // Scan newly-emitted dispatch requests for charge/discharge power.
        // A rate dispatched this step is the driver's own command and takes
        // precedence. Goal-based strategies dispatch no rate; the equipment
        // BMS applies its own power, observed via `observed_charge_kw`.
        let mut charge_kw: Option<f64> = None;
        let mut discharge_min_soc = 0.0;
        for req in &out[before_out..] {
            match &req.signal {
                ControlSignal::PowerSetpoint {
                    active_power_kw,
                    min_soc,
                    ..
                } => {
                    charge_kw = Some(*active_power_kw);
                    // Why: None min_soc on a PowerSetpoint means "no discharge
                    // floor constraint" — only V2G/V2H strategies set min_soc.
                    // 0.0 = "allow full discharge" is the default operational
                    // behaviour, so the sentinel is semantically correct.
                    discharge_min_soc = min_soc.unwrap_or(0.0);
                }
                ControlSignal::EvAwayCharge { power_kw } => charge_kw = Some(*power_kw),
                ControlSignal::EvDrive { .. } => {} // driving, not charging
                _ => {}
            }
        }
        let charge_kw = charge_kw
            .or_else(|| self.observed_charge_kw(env))
            .unwrap_or(0.0);
        self.telemetry.set("charge_kw", charge_kw);
        self.telemetry.set("discharge_min_soc", discharge_min_soc);
        self.telemetry.set(
            "soc_gate_charging_allowed",
            if self.composer.soc_gate_charging_allowed() {
                1.0
            } else {
                0.0
            },
        );
        self.telemetry.set(
            "needed_charge_hours",
            self.composer.last_needed_charge_hours(),
        );
        // The driver-is-short signal: 1.0 while perceived SOC sits below the
        // anxiety band, regardless of whether the urgency gate has fired —
        // predictive state (see the channel's init comment for the rationale).
        self.telemetry.set(
            "range_anxiety_active",
            if self.range_anxiety_miles > 0.0 && self.below_anxiety_band(env) {
                1.0
            } else {
                0.0
            },
        );
        self.telemetry
            .set("drive_cancelled", f64::from(self.drive_cancelled));
    }

    /// Returns the target equipment name.
    pub fn target_name(&self) -> &str {
        match &self.dispatch_target {
            DispatchTarget::ByName(n) => n,
            DispatchTarget::ByEndUse(_) => unreachable!("EvDriverActor always targets by name"),
        }
    }

    /// Roll a daily event for a new day if needed.
    fn maybe_roll_daily_event(&mut self, env: &EnvironmentState) {
        let ordinal = env.current_time.ordinal0() as i32 + env.current_time.year() * 366;
        if ordinal == self.current_day_ordinal {
            return;
        }
        self.current_day_ordinal = ordinal;

        // Reconcile estimated_soc at day start when plugged in. Overnight grid
        // charging can change actual SOC without a new plug-in event — the
        // driver observes the battery level when waking up. The arrival
        // reconciliation (above) covers the plug-in event itself; this covers
        // multi-day stay-at-home scenarios.
        if matches!(self.phase, DriverPhase::HomePluggedIn) {
            self.reconcile_soc(env, "day_start");
        }

        // Decide if today is a driving day
        let roll: f64 = self.rng.random();
        if roll >= self.event_day_ratio {
            self.todays_event = None;
            return;
        }

        // Sample departure time from ScheduleSource (seeded, reproducible)
        let departure_min = self
            .departure_time
            .value_at(env)
            .unwrap_or(480.0)
            .clamp(0.0, 1439.0) as u16;

        let arrival = if let Some(ref mut arrival_src) = self.arrival_time {
            // Direct arrival sampling: arrival is drawn independently.
            // Re-sample up to 5 times if arrival <= departure to preserve
            // the constraint arrival > departure without altering the
            // unconditional arrival distribution shape.
            let mut arrival_raw = arrival_src
                .value_at(env)
                .unwrap_or(1080.0)
                .clamp(0.0, 1439.0) as u16;
            for _ in 0..5 {
                if arrival_raw > departure_min {
                    break;
                }
                arrival_raw = arrival_src
                    .value_at(env)
                    .unwrap_or(1080.0)
                    .clamp(0.0, 1439.0) as u16;
            }
            // Guard: if re-sampling fails (extremely unlikely given the
            // ~540-minute gap between commuter departure and arrival means),
            // clamp to departure+1 to keep the invariant.
            if arrival_raw <= departure_min {
                (departure_min as u32 + 1).min(1439) as u16
            } else {
                arrival_raw
            }
        } else {
            // Fallback: derive arrival from departure + trip duration.
            let dur = self
                .trip_duration
                .value_at(env)
                .unwrap_or(600.0)
                .clamp(30.0, 1200.0) as u16;
            (departure_min as u32 + dur as u32).min(1439) as u16
        };

        // Sample daily miles from the ScheduleSource
        let miles = self
            .daily_drive_miles
            .value_at(env)
            .unwrap_or(30.0)
            .max(0.0);

        #[cfg(feature = "observe")]
        {
            // Log raw sampled miles per vehicle-day for distributional validation.
            tracing::debug!(
                actor = %self.name,
                raw_miles = miles,
                day_ordinal = self.current_day_ordinal,
                "EV daily miles sample"
            );
            if miles < 1.0 {
                tracing::debug!(
                    actor = %self.name,
                    miles = miles,
                    day_ordinal = self.current_day_ordinal,
                    "EV daily miles below 1-mi diagnostic threshold"
                );
            }
            // Emit departure, arrival, and derived duration as diagnostic
            // columns per vehicle-day for distributional validation.
            let dur = (arrival as u32).saturating_sub(departure_min as u32) as u16;
            let kind = if self.arrival_time.is_some() {
                "direct"
            } else {
                "derived"
            };
            tracing::debug!(
                actor = %self.name,
                departure_minute = departure_min,
                arrival_minute = arrival,
                duration_minutes = dur,
                arrival_distribution_kind = kind,
                day_ordinal = self.current_day_ordinal,
                "EV driver daily schedule sample"
            );
        }
        let temp_multiplier = temp_efficiency_multiplier(env.weather.outdoor_temp_c);
        let drive_kwh = miles * self.fuel_economy_kwh_per_mi * temp_multiplier;

        self.todays_event = Some(DayEvent {
            departure_minute: departure_min,
            arrival_minute: arrival,
            drive_kwh,
        });
    }

    /// Check if current minute-of-day matches a target minute within time resolution.
    fn minute_matches(&self, current_minute: u16, target_minute: u16) -> bool {
        let res = self.time_res_minutes.max(1.0) as u16;
        current_minute >= target_minute && current_minute < target_minute.saturating_add(res)
    }

    /// The pack's available drive energy [kWh], from the best observable
    /// source. This is an energy-transfer bound, not a behavioral decision
    /// (which the module doc keeps on the driver's belief): the physical
    /// pack cannot leave the driveway with more energy than it holds.
    ///
    /// Ladder: observed SOC × the equipment's published
    /// (degradation-adjusted) capacity when both channels are live — exact,
    /// the same arithmetic the equipment's `EvDrive` guard uses; observed
    /// SOC × this actor's rated capacity when only SOC is observable (an
    /// over-estimate under pack degradation, never an under-estimate — the
    /// residual is handled by the equipment's truncation accounting); the
    /// driver's belief when nothing is observable (no equipment is
    /// registered, so nothing can reject a dispatch).
    fn observed_pack_kwh(&self, env: &EnvironmentState) -> f64 {
        // allowed: capacity_kwh is the equipment's degradation-adjusted
        // pack capacity, telemetry-only (CoreOutput has no capacity
        // field); the same contract the dwelling's EV-capacity invariant
        // checker reads it under. equipment_telemetry is name-keyed, so
        // the lookup is by the dispatch target's name.
        let capacity = env
            .equipment_telemetry
            .get(self.target_name())
            .and_then(|t| t.get(tk::CAPACITY_KWH))
            .unwrap_or(self.capacity_kwh);
        self.actual_soc(env)
            .map(|soc| soc * capacity)
            .unwrap_or_else(|| self.perceived_soc() * self.capacity_kwh.max(0.01))
    }

    /// The SOC the driver's strategy usually keeps the pack at — the
    /// "usual needed amount" a short driver charges back up to, whether at
    /// home (the strategy's own target, once back above the anxiety band)
    /// or out (the away top-up). Mirrors the `SocTarget` each variant's
    /// stack installs in `build_preferences`; keep the two in step when a
    /// strategy variant changes its target.
    fn usual_target_soc(&self) -> f64 {
        match &self.strategy {
            ChargingStrategy::Immediate { target_soc }
            | ChargingStrategy::Nightly { target_soc, .. }
            | ChargingStrategy::LowSoc { target_soc, .. }
            | ChargingStrategy::PreDeparture { target_soc, .. }
            | ChargingStrategy::TouAware { target_soc, .. } => *target_soc,
            ChargingStrategy::QuickThenWait { partial_soc } => *partial_soc,
            // SolarSurplus's stack carries no explicit target; its
            // DepartureDeadline charges toward full.
            ChargingStrategy::SolarSurplus { .. } => 1.0,
            // V2H/V2G stacks charge toward 0.9 / 1.0 respectively (see
            // `build_preferences`).
            ChargingStrategy::V2H { .. } => 0.9,
            ChargingStrategy::V2G { .. } => 1.0,
        }
    }

    /// The actor's perceived SOC — its best guess diverging from actual equipment
    /// SOC because the actor doesn't observe CC-CV taper, thermal derating, or BMS
    /// charge termination. This is the value used for all driver behavioral decisions.
    fn perceived_soc(&self) -> f64 {
        self.estimated_soc
    }

    /// Read the ground-truth SOC from typed equipment core output.
    ///
    /// Returns `None` when the equipment is not registered (e.g. test scenarios).
    /// For behavioral decisions use `perceived_soc()`. This function exists for
    /// reconciliation, observability, and diagnostics — never for driver logic.
    fn actual_soc(&self, env: &EnvironmentState) -> Option<f64> {
        self.equipment_id
            .and_then(|id| env.equipment_core.get(&id))
            .and_then(|co| co.state.soc)
            .map(|soc| soc.get())
    }

    /// Reconcile estimated_soc to actual equipment SOC.
    ///
    /// Called at plug-in and day-start to align the driver's energy-accounting
    /// estimate with the equipment BMS's measured state of charge. This closes
    /// the drift gap that accumulates during driving when the driver
    /// overestimates discharge and underestimates charge.
    fn reconcile_soc(&mut self, env: &EnvironmentState, _event_label: &str) {
        if let Some(actual) = self.actual_soc(env) {
            #[cfg(feature = "observe")]
            {
                let pre = self.estimated_soc;
                tracing::debug!(
                    actor = %self.name,
                    pre_reconcile_soc = pre,
                    post_reconcile_soc = actual,
                    actual_soc = actual,
                    correction = actual - pre,
                    reconcile_event = _event_label,
                    "EV driver SOC reconciliation"
                );
            }
            self.estimated_soc = actual;
        }
    }

    /// Should the driver plug in at home based on perceived SOC and policy.
    fn should_plug_in(&self) -> bool {
        match &self.plug_in_policy {
            PlugInPolicy::Always => true,
            PlugInPolicy::LowSoc { threshold } => self.perceived_soc() < *threshold,
        }
    }

    /// The SOC below which the day's expected driving plus the
    /// range-anxiety buffer would strand the driver, and the mile figure the
    /// threshold was computed from. Single home for the anxiety-band
    /// arithmetic: the trigger check, its observe diagnostics, and the
    /// override's urgency gate must all read the same band.
    ///
    /// Computes from the day's actual drive_kwh when available, blended via
    /// `max(day_specific, expected_daily_miles)` so the threshold is never
    /// lower than the static mean — minimum protection even on below-average
    /// days. Falls back to `expected_daily_miles` on non-driving days.
    fn anxiety_band(&self, env: &EnvironmentState) -> (f64, f64) {
        let ambient_c = env.weather.outdoor_temp_c;
        let temp_mult = temp_efficiency_multiplier(ambient_c);

        let day_specific_miles = match self.todays_event {
            Some(event) => event.drive_kwh / self.fuel_economy_kwh_per_mi.max(0.01),
            None => self.expected_daily_miles,
        };

        let miles_for_anxiety = day_specific_miles.max(self.expected_daily_miles);

        #[cfg(debug_assertions)]
        {
            if let Some(event) = self.todays_event {
                let trip_miles = event.drive_kwh / self.fuel_economy_kwh_per_mi.max(0.01);
                debug_assert!(
                    (miles_for_anxiety - trip_miles.max(self.expected_daily_miles)).abs() < 1e-9,
                    "anxiety miles must be computed from day-specific drive_kwh \
                     (blended with expected_daily_miles via max)"
                );
            }
        }

        let anxiety_kwh = (miles_for_anxiety + self.range_anxiety_miles)
            * self.fuel_economy_kwh_per_mi
            * temp_mult;
        let anxiety_soc = anxiety_kwh / self.capacity_kwh.max(0.01);
        (anxiety_soc, miles_for_anxiety)
    }

    /// Check if tomorrow's expected trip would leave perceived SOC dangerously low.
    /// If so, the driver overrides their strategy and charges (minimally, to
    /// the band — see `maybe_push_range_anxiety_override`).
    fn needs_range_anxiety_override(&self, env: &EnvironmentState) -> bool {
        if self.range_anxiety_miles <= 0.0 {
            return false;
        }
        self.below_anxiety_band(env)
    }

    /// Whether the driver's perceived SOC sits below the range-anxiety band
    /// (tomorrow's trip plus the safety buffer) — the "driver is short"
    /// state. Single home for the band comparison: the override trigger, its
    /// observe diagnostics, and the `range_anxiety_active` telemetry channel
    /// must all read the same band.
    fn below_anxiety_band(&self, env: &EnvironmentState) -> bool {
        let soc = self.perceived_soc();
        // The mile figure feeds only the observe diagnostics below;
        // underscore binding per the module's convention for observe-only
        // values (see `reconcile_soc`'s `_event_label`).
        let (anxiety_soc, _miles_for_anxiety) = self.anxiety_band(env);

        #[cfg(feature = "observe")]
        {
            let day_drive_kwh = self.todays_event.map(|e| e.drive_kwh).unwrap_or(0.0);
            let triggered = soc < anxiety_soc;
            // `plugged_in` is derived from the plug-in policy at the current SOC,
            // not from `DriverPhase`: the phase conflates "at home" with
            // "physically connected", so it cannot answer "is the vehicle
            // plugged in". `should_plug_in()` is the observable proxy.
            // `non_driving_day` distinguishes the non-driving-day override call
            // site (decide's None arm) from the normal in-phase evaluation.
            tracing::debug!(
                actor = %self.name,
                anxiety_soc,
                day_drive_kwh,
                miles_term = _miles_for_anxiety,
                expected_daily_miles = self.expected_daily_miles,
                range_anxiety_miles = self.range_anxiety_miles,
                perceived_soc = soc,
                non_driving_day = self.todays_event.is_none(),
                plugged_in = self.should_plug_in(),
                triggered,
                "EV driver range anxiety evaluation"
            );
        }

        soc < anxiety_soc
    }

    /// The range-anxiety charging override for this step — a minimal-charge
    /// `SOCTarget` at the anxiety band — when the next trip would strand the
    /// driver, or `None` when the override does not fire.
    ///
    /// Four conditions, in order:
    ///
    /// 1. `needs_range_anxiety_override` — perceived SOC is below the
    ///    anxiety band (the next trip plus the range-anxiety buffer).
    ///
    /// 2. The strategy's own resolved target for this step (the composer's
    ///    in-window plan, read via `last_resolved_target_soc`) is at or
    ///    above the band, **the plan's effective ceiling cannot stop below
    ///    the band** (the resolved `max_soc` cap, defaulting to the target
    ///    itself when the plan carries none), **and the resolved plan
    ///    actually charges toward it** (the resolved rate is absent — a
    ///    target-only plan, which charges at the equipment's own rate — or
    ///    positive): the band is already covered, the strategy governs, and
    ///    the override stands down. A resolved rate that is zero or negative
    ///    means the plan is holding or *discharging* — a rate-bearing
    ///    strategy (TouAware at peak price, V2G exporting) can pair a
    ///    ceiling at or above the band with a discharge rate, and the pack
    ///    is then being driven away from the band, not toward it; a resolved
    ///    `max_soc` under the band is the same geometry in the cap
    ///    dimension — the equipment halts charging at the cap while the
    ///    target reads high. Either way, standing down silently disables the
    ///    backstop the ticket names ("at minimum the range-anxiety backstop
    ///    charges an empty pack"). `None` target means no target to protect
    ///    (outside the window, a rate-only plan, an idle hold) and the
    ///    override proceeds to the urgency gate.
    /// 3. An urgency gate, when today's trip is known: the override exists to
    ///    prevent stranding, not to preempt the driver's configured strategy
    ///    whenever SOC happens to sit inside the band. When the known
    ///    departure leaves enough time to reach the band — with
    ///    the same 20% safety margin `DepartureDeadline`'s urgency override
    ///    uses — the strategy (window, gate, price) governs and the override
    ///    stands down until time actually runs short. With no known trip
    ///    today there is no "later" to defer to, so the override fires on the
    ///    band condition alone.
    ///
    /// 4. The target is the band itself — a *minimal* charge to the next
    ///    trip plus a safe reserve, not a charge to full: a driver who is
    ///    short tops up just enough to make the trip safely, then their
    ///    usual pattern takes over again (the strategy's own target governs
    ///    once the pack is back above the band). Charging to full here would
    ///    instead make every anxious night a full-rate, full-pack session.
    ///
    /// This is the single home for the override rule: both the in-phase
    /// charging path (`evaluate_charging`) and the non-driving-day path
    /// (`decide`'s `None` arm, via `evaluate_charging`) route through here so
    /// the charging push exists in exactly one place.
    ///
    /// `Schedule` tier — the EV driver is a schedule-level actor; this override is
    /// a pre-defined operational rule, not a user or grid action.
    fn range_anxiety_override_request(
        &self,
        env: &EnvironmentState,
        current_minute: u16,
    ) -> Option<DispatchRequest> {
        if !self.needs_range_anxiety_override(env) {
            return None;
        }
        let (anxiety_soc, _) = self.anxiety_band(env);
        // The strategy's own resolved target for this step governs when it
        // already covers the band, the plan cannot stop below the band, AND
        // the resolved plan charges toward it: pre-empting a higher active
        // target with a lower one charges less than the driver configured
        // for zero safety gain. The rate check is what makes "charges toward
        // it" true — a target-only plan (rate `None`) charges toward the
        // target at the equipment's own rate, and a positive rate charges
        // explicitly, but a zero or negative rate holds or discharges. The
        // cap check is what makes "cannot stop below" true — a resolved
        // `max_soc` under the band halts charging at the cap while the
        // target reads high, so the effective ceiling is the cap, not the
        // target; a plan with no cap defaults the ceiling to the target,
        // which the first clause already proved covers the band. `None`
        // target (outside the window, a rate-only plan, an idle hold) means
        // no target to protect and the override proceeds to the urgency
        // gate.
        if let Some(strategy_target) = self.composer.last_resolved_target_soc()
            && strategy_target >= anxiety_soc
            && self
                .composer
                .last_resolved_power_kw()
                .is_none_or(|power| power > 0.0)
            && self
                .composer
                .last_resolved_max_soc()
                .is_none_or(|cap| cap >= anxiety_soc)
        {
            return None;
        }
        if let Some(event) = self.todays_event {
            let hours_left =
                minutes_until(current_minute, u32::from(event.departure_minute)) / 60.0;
            let ctx = self.decision_context(env, current_minute);
            let needed = needed_charge_hours_to_target(anxiety_soc, CHARGING_EFFICIENCY, &ctx);
            if hours_left >= needed * 1.2 {
                return None;
            }
        }
        Some(DispatchRequest {
            target: self.dispatch_target.clone(),
            signal: ControlSignal::SOCTarget {
                target_soc: anxiety_soc.clamp(0.0, 1.0),
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        })
    }

    /// Build the per-step decision context for **dispatch**: `current_soc`
    /// is the driver's perceived SOC — control runs on the driver's belief,
    /// and changing that would alter dispatch, which the module doc
    /// explicitly forbids.
    fn decision_context<'a>(
        &self,
        env: &'a EnvironmentState,
        current_minute: u16,
    ) -> DecisionContext<'a> {
        DecisionContext {
            current_soc: self.perceived_soc(),
            capacity_kwh: self.capacity_kwh,
            max_charge_kw: self.max_charge_kw,
            max_discharge_kw: self.max_charge_kw,
            env,
            current_minute,
            next_departure_minute: self.todays_event.map(|e| e.departure_minute),
            time_res_minutes: self.time_res_minutes,
            observed_charge_derate: self.observed_charge_derate(env),
        }
    }

    /// The equipment's own published cold-charge capability factor
    /// (`CHARGE_DERATE` telemetry) when the target equipment is
    /// observable — the physical state the needed-hours estimate keys on
    /// (see `DecisionContext::observed_charge_derate`). Not a driver
    /// belief: observing the equipment's capability does not touch the
    /// belief-vs-observed SOC contract that dispatch control runs on.
    fn observed_charge_derate(&self, env: &EnvironmentState) -> Option<f64> {
        // allowed: CHARGE_DERATE is the equipment's published temperature
        // capability, telemetry-only (CoreOutput has no derate field); the
        // same name-keyed telemetry contract `observed_pack_kwh` reads
        // under.
        env.equipment_telemetry
            .get(self.target_name())
            .and_then(|t| t.get(tk::CHARGE_DERATE))
    }

    /// Build the context that feeds the `needed_charge_hours` **telemetry**
    /// fold: the decision context with the observed equipment SOC
    /// substituted whenever the equipment is observable — the channel
    /// reports the observed battery gap (the same face as the `soc`
    /// channel), so it tracks the shrinking gap during a charging session
    /// instead of freezing at the driver's belief, which only moves on
    /// driving steps and the arrival/day-start reconciliations. Falls back
    /// to the belief in the warned unresolved-equipment mode, exactly like
    /// the `soc` channel.
    fn telemetry_estimate_context<'a>(
        &self,
        env: &'a EnvironmentState,
        current_minute: u16,
    ) -> DecisionContext<'a> {
        let mut ctx = self.decision_context(env, current_minute);
        if let Some(observed) = self.actual_soc(env) {
            ctx.current_soc = observed;
        }
        ctx
    }

    /// Record the needed-charge-hours estimate for the range-anxiety
    /// override's plan (minimal charge to the band), replacing the
    /// standing-strategy fold refreshed at step start. Called on the paths
    /// where the override replaces the preference stack. Runs on the
    /// observed SOC — the same telemetry face as the fold it replaces.
    fn record_anxiety_plan_hours(&mut self, env: &EnvironmentState, current_minute: u16) {
        let ctx = self.telemetry_estimate_context(env, current_minute);
        let (anxiety_soc, _) = self.anxiety_band(env);
        let hours =
            needed_charge_hours_to_target(anxiety_soc.clamp(0.0, 1.0), CHARGING_EFFICIENCY, &ctx);
        // +∞ (charging physically impossible right now — observed derate 0,
        // preconditioning in progress) cannot cross the telemetry channel's
        // finiteness contract; publish the ambient-curve fallback for the
        // same gap, the same encoding `Composer::refresh_needed_charge_hours`
        // uses. The override's own decision logic compares the true estimate
        // directly and treats ∞ as maximum urgency.
        let hours = if hours.is_finite() {
            hours
        } else {
            needed_charge_hours_to_target(
                anxiety_soc.clamp(0.0, 1.0),
                CHARGING_EFFICIENCY,
                &DecisionContext {
                    observed_charge_derate: None,
                    ..ctx.clone()
                },
            )
        };
        self.composer.set_needed_charge_hours(hours);
    }

    /// Evaluate the composer per-step while plugged in at home.
    ///
    /// Precedence: the composer's plan for this step is resolved *first*, so
    /// the range-anxiety override's decision
    /// (`range_anxiety_override_request`) compares against the strategy's
    /// own active target instead of pre-empting it blind. Path exclusivity
    /// is preserved exactly: a step's charging dispatch comes from either
    /// the override (the composer's signals for that step are dropped) or
    /// the composer (already appended below) — never both, never neither.
    fn evaluate_charging(
        &mut self,
        env: &EnvironmentState,
        current_minute: u16,
        out: &mut Vec<DispatchRequest>,
    ) {
        let ctx = self.decision_context(env, current_minute);
        let composer_start = out.len();
        self.composer.evaluate(&ctx, out);

        if let Some(request) = self.range_anxiety_override_request(env, current_minute) {
            // The override pre-empts the composer for this step: drop the
            // composer's dispatches and keep only the override's
            // minimal-charge plan.
            out.truncate(composer_start);
            out.push(request);
            // The override records the estimate for its own target (the
            // band) in place of the strategy-plan fold refreshed at step
            // start.
            self.record_anxiety_plan_hours(env, current_minute);
            return;
        }

        tracing::trace!(
            actor = %self.name,
            last_action = self.composer.last_action(),
            "ev charging decision",
        );
    }
}

/// Serializable snapshot of EvDriverActor mutable runtime state for checkpointing.
#[derive(Serialize, Deserialize)]
struct EvDriverSnapshot {
    estimated_soc: f64,
    current_day_ordinal: i32,
    todays_event: Option<DayEvent>,
    phase: DriverPhase,
    rng_seed: [u8; 32],
    rng_stream: u64,
    rng_word_pos: u128,
    needs_away_charge: bool,
    /// Cumulative cancelled-trip count; see `EvDriverActor::drive_cancelled`.
    drive_cancelled: u32,
}

impl Actor for EvDriverActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
    }

    fn dispatch_target_name(&self) -> Option<&str> {
        Some(self.target_name())
    }

    fn resolve_equipment_id(&mut self, equipment_id_by_name: &HashMap<String, EquipmentId>) {
        self.equipment_id = match equipment_id_by_name.get(self.target_name()) {
            Some(id) => Some(*id),
            None => {
                tracing::warn!(
                    equipment = %self.target_name(),
                    actor = "EvDriverActor",
                    "Equipment name not found in registry — actor will operate without SOC feedback"
                );
                None
            }
        };
    }

    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        // All signals emitted by this actor use `Schedule` tier.
        // The EV driver executes a pre-defined driving/charging schedule
        // — it is not a user override or grid DR actor. The tier matches
        // the central `From<&ControlSignal> for PriorityTier` mapping for
        // EvPlugIn, EvDrive, EvAwayCharge, EvSetReadyBy, and SOCTarget.
        let res_seconds = env.time_res.num_seconds();
        debug_assert!(res_seconds >= 1, "time_res must be >= 1 second");
        self.time_res_minutes = (res_seconds.max(1) as f64) / 60.0;
        self.maybe_roll_daily_event(env);

        let before_out = out.len();
        let current_minute = (env.current_time.hour() * 60 + env.current_time.minute()) as u16;

        // The needed-charge-hours channel promises an estimate from the
        // current battery state on every step, in every phase, so it is
        // refreshed once per step — before any phase arm runs — instead of
        // on the arms that happen to dispatch. The refresh runs on the
        // *observed* SOC (telemetry truth); dispatch contexts keep the
        // driver's belief. Without this, the channel freezes at the last
        // plugged-in value while driving/away, and the first step after a
        // checkpoint restore — or the first-ever step of a fresh actor —
        // publishes the composer's "no estimate" sentinel, which collapses
        // to 0.0. Paths whose active plan is the range-anxiety override
        // replace this value with the override's own (anxiety-band)
        // estimate.
        let ctx = self.telemetry_estimate_context(env, current_minute);
        self.composer.refresh_needed_charge_hours(&ctx);

        // A non-driving day changes only *future* departures. An in-flight
        // trip — mid-route, or parked away awaiting its own arrival minute —
        // is phase-carried state from the departure day: freezing those
        // phases because today rolled non-driving would stall a
        // midnight-crossing trip mid-route until the next driving day, with
        // no driving steps dispatched, no away charging, and no arrival.
        // The Driving and Away arms below therefore run on their carried
        // plan regardless of `todays_event`; only the home arm consults it.
        match self.phase {
            DriverPhase::HomePluggedIn => {
                let Some(event) = self.todays_event else {
                    // A non-driving day still charges: route through the same
                    // `evaluate_charging` the driving-day path uses, so the
                    // composer's decision — a hold while the strategy idles, a
                    // target while it charges, or the range-anxiety override,
                    // which `evaluate_charging` tries first, unchanged — is
                    // asserted on every plugged-in step. Returning after the
                    // override check alone would leave a hold latched from the
                    // previous evening in force through the entire day, so a
                    // window strategy would miss its own window on the ~20% of
                    // days with no trip. Only meaningful while at home: an Away
                    // vehicle cannot plug into the home charger.
                    self.evaluate_charging(env, current_minute, out);
                    self.populate_telemetry(env, before_out, out);
                    return;
                };
                if self.minute_matches(current_minute, event.departure_minute) {
                    // A trip the pack cannot cover is cancelled, not driven
                    // short: a driver who cannot complete the trip does not
                    // depart and drive until the pack dies mid-route — they
                    // stay home (another mode, another day) and charge. The
                    // vehicle remains plugged in and recovers overnight
                    // under its own strategy (with the range-anxiety backstop
                    // if the strategy cannot cover it), so the day self-heals;
                    // the cancellation is counted in `drive_cancelled` so a
                    // shortfall is reported, never silently dropped mobility.
                    let pack_kwh = self.observed_pack_kwh(env);
                    if event.drive_kwh > pack_kwh {
                        self.drive_cancelled += 1;
                        // Consume the day's event: the trip is cancelled, not
                        // deferred — a second step landing inside the
                        // departure-minute window (possible with a start
                        // time not aligned to the time resolution) must not
                        // count the same trip twice, and the rest of the day
                        // is a non-driving day for the EV (the band falls
                        // back to expected miles, the honest basis for a
                        // trip that will not happen).
                        self.todays_event = None;
                        tracing::warn!(
                            actor = %self.name,
                            target = self.target_name(),
                            requested_drive_kwh = event.drive_kwh,
                            available_pack_kwh = pack_kwh,
                            "EV trip cancelled — the pack cannot cover the day's trip; the \
                             driver stays home and charges"
                        );
                        self.populate_telemetry(env, before_out, out);
                        return;
                    }

                    // Departure: disconnect
                    out.push(DispatchRequest {
                        target: self.dispatch_target.clone(),
                        signal: ControlSignal::EvPlugIn {
                            state: EvConnectionState::Disconnected,
                        },
                        priority: PriorityTier::Schedule,
                    });

                    // Start multi-step driving
                    let trip_hours = if self.average_speed_mph > 0.0 {
                        let miles = event.drive_kwh / self.fuel_economy_kwh_per_mi.max(0.01);
                        miles / self.average_speed_mph
                    } else {
                        0.0
                    };
                    let trip_minutes = (trip_hours * 60.0).max(self.time_res_minutes);
                    let total_steps = (trip_minutes / self.time_res_minutes).ceil().max(1.0) as u32;

                    self.phase = DriverPhase::Driving {
                        remaining_kwh: event.drive_kwh,
                        total_steps,
                        steps_done: 0,
                        plan: event,
                    };

                    tracing::debug!(
                        actor = %self.name,
                        target = self.target_name(),
                        departure_minute = event.departure_minute,
                        drive_kwh = event.drive_kwh,
                        total_steps,
                        "EV driver departing",
                    );
                } else {
                    self.evaluate_charging(env, current_minute, out);
                }
            }
            DriverPhase::Driving {
                remaining_kwh,
                total_steps,
                steps_done,
                plan,
            } => {
                let steps_left = total_steps.saturating_sub(steps_done);
                if steps_left == 0 {
                    self.phase = DriverPhase::Away { plan };
                    self.populate_telemetry(env, before_out, out);
                    return;
                }

                // Spread energy evenly across remaining steps
                let kwh_this_step = remaining_kwh / steps_left as f64;

                out.push(DispatchRequest {
                    target: self.dispatch_target.clone(),
                    signal: ControlSignal::EvDrive { kwh: kwh_this_step },
                    priority: PriorityTier::Schedule,
                });

                self.estimated_soc =
                    (self.estimated_soc - kwh_this_step / self.capacity_kwh.max(0.01)).max(0.0);

                let new_steps_done = steps_done + 1;
                let new_remaining = remaining_kwh - kwh_this_step;

                if new_steps_done >= total_steps {
                    // Trip complete -- defer away-charge signals to the next
                    // step so EvDrive is fully processed before EvPlugIn.
                    if self.away_charge_fraction > 0.0 {
                        // Habitual away top-up: a fraction of the day's drive.
                        // But a driver who arrives short of their usual
                        // amount charges out back up to it — the full
                        // top-up, not the habitual fraction — and then skips
                        // the home charge cycle (arriving at the usual
                        // target, the evening's home session transfers
                        // nothing). Take whichever recoup is larger.
                        let habitual_kwh = plan.drive_kwh * self.away_charge_fraction;
                        let usual_kwh = self.usual_target_soc() * self.capacity_kwh.max(0.01);
                        let short_of_usual_kwh = (usual_kwh
                            - self.perceived_soc() * self.capacity_kwh.max(0.01))
                        .max(0.0);
                        let recoup_kwh = habitual_kwh.max(short_of_usual_kwh);
                        let recoup_soc = recoup_kwh / self.capacity_kwh.max(0.01);
                        self.estimated_soc = (self.estimated_soc + recoup_soc).min(1.0);
                        self.needs_away_charge = true;
                    }
                    self.phase = DriverPhase::Away { plan };
                } else {
                    self.phase = DriverPhase::Driving {
                        remaining_kwh: new_remaining,
                        total_steps,
                        steps_done: new_steps_done,
                        plan,
                    };
                }
            }
            DriverPhase::Away { plan } => {
                // Emit deferred away-charge signals from the previous
                // driving step (separated so EvDrive processes first).
                if self.needs_away_charge {
                    self.needs_away_charge = false;
                    out.push(DispatchRequest {
                        target: self.dispatch_target.clone(),
                        signal: ControlSignal::EvPlugIn {
                            state: EvConnectionState::AwayPluggedIn,
                        },
                        priority: PriorityTier::Schedule,
                    });
                    // Bound the away session at the driver's usual amount:
                    // "charge out back up to their usual needed amount" — a
                    // top-up, not a fill-to-BMS-default. The target is
                    // cleared again on the away disconnect, so it never
                    // leaks into the next home session.
                    out.push(DispatchRequest {
                        target: self.dispatch_target.clone(),
                        signal: ControlSignal::SOCTarget {
                            target_soc: self.usual_target_soc(),
                            min_soc: None,
                            max_soc: None,
                        },
                        priority: PriorityTier::Schedule,
                    });
                    out.push(DispatchRequest {
                        target: self.dispatch_target.clone(),
                        signal: ControlSignal::EvAwayCharge {
                            power_kw: self.away_charge_power_kw,
                        },
                        priority: PriorityTier::Schedule,
                    });
                }

                if self.minute_matches(current_minute, plan.arrival_minute) {
                    // Disconnect from away charger if active (must go through
                    // Disconnected before HomePluggedIn per EV transition rules)
                    if self.away_charge_fraction > 0.0 {
                        out.push(DispatchRequest {
                            target: self.dispatch_target.clone(),
                            signal: ControlSignal::EvPlugIn {
                                state: EvConnectionState::Disconnected,
                            },
                            priority: PriorityTier::Schedule,
                        });
                    }

                    let doing_plugin = self.should_plug_in();
                    if doing_plugin {
                        out.push(DispatchRequest {
                            target: self.dispatch_target.clone(),
                            signal: ControlSignal::EvPlugIn {
                                state: EvConnectionState::HomePluggedIn,
                            },
                            priority: PriorityTier::Schedule,
                        });
                    }

                    self.phase = DriverPhase::HomePluggedIn;

                    // Reconcile the driver's energy-accounting estimate to the
                    // equipment BMS measured SOC. On arrival the driver observes
                    // the actual battery state and updates their belief.
                    self.reconcile_soc(env, "arrival");

                    // The arrival step itself must carry a charging decision.
                    // The plug-in dispatch above is applied before the
                    // equipment steps this timestep, so evaluating here —
                    // rather than first on the *next* step — closes what would
                    // otherwise be one full step of charging under whatever
                    // setpoint was last latched, outside whatever window the
                    // strategy configures. Skipped when the plug-in policy
                    // keeps the vehicle unplugged: there is no charger to
                    // instruct, and the next plugged-in step decides.
                    if doing_plugin {
                        self.evaluate_charging(env, current_minute, out);
                    }

                    tracing::debug!(
                        actor = %self.name,
                        target = self.target_name(),
                        arrival_minute = plan.arrival_minute,
                        estimated_soc = self.estimated_soc,
                        plugged_in = self.should_plug_in(),
                        "EV driver arrived home",
                    );
                }
            }
        }

        // Observability and diagnostics: record divergence between perceived SOC
        // (the actor's internal estimate) and actual equipment core SOC.
        #[cfg(feature = "observe")]
        {
            let actual = self.actual_soc(env);
            if let Some(a) = actual {
                let divergence = (a - self.estimated_soc).abs();
                tracing::debug!(
                    actor = %self.name,
                    estimated_soc = self.estimated_soc,
                    actual_soc = a,
                    divergence = divergence,
                    "EV driver SOC divergence"
                );
            }
        }

        // Invariant: when equipment core telemetry is available, the actual SOC
        // should be within a reasonable band of the estimated SOC. The 15% band
        // is generous — real BMS limits, CC-CV taper, and thermal derating
        // should not push divergence beyond this in normal operation. A breach
        // signals either a modelling error (too-aggressive fade) or an estimator
        // bug (e.g. missing away-charge credit).
        #[cfg(debug_assertions)]
        {
            let actual = self.actual_soc(env);
            if let Some(a) = actual {
                let divergence = (a - self.estimated_soc).abs();
                if divergence > 0.15 {
                    tracing::warn!(
                        actor = %self.name,
                        estimated_soc = self.estimated_soc,
                        actual_soc = a,
                        divergence = divergence,
                        "EV driver SOC estimate diverged >15% from actual equipment SOC"
                    );
                }
            }
        }

        self.populate_telemetry(env, before_out, out);
    }

    fn save_state(&self) -> Result<Vec<u8>, HaresError> {
        let snap = EvDriverSnapshot {
            estimated_soc: self.estimated_soc,
            current_day_ordinal: self.current_day_ordinal,
            todays_event: self.todays_event,
            phase: self.phase,
            rng_seed: self.rng.get_seed(),
            rng_stream: self.rng.get_stream(),
            rng_word_pos: self.rng.get_word_pos(),
            needs_away_charge: self.needs_away_charge,
            drive_cancelled: self.drive_cancelled,
        };
        postcard::to_allocvec(&snap)
            .map_err(|e| HaresError::Io(format!("EvDriverActor save_state: {e}")))
    }

    fn load_state(&mut self, data: &[u8]) -> Result<(), HaresError> {
        if data.is_empty() {
            return Ok(());
        }
        let snap: EvDriverSnapshot = postcard::from_bytes(data)
            .map_err(|e| HaresError::Io(format!("EvDriverActor load_state: {e}")))?;
        self.estimated_soc = snap.estimated_soc;
        self.current_day_ordinal = snap.current_day_ordinal;
        self.todays_event = snap.todays_event;
        self.phase = snap.phase;
        let mut rng = ChaCha8Rng::from_seed(snap.rng_seed);
        rng.set_stream(snap.rng_stream);
        rng.set_word_pos(snap.rng_word_pos);
        self.rng = rng;
        self.needs_away_charge = snap.needs_away_charge;
        self.drive_cancelled = snap.drive_cancelled;
        Ok(())
    }

    /// v2: `DriverPhase::Driving`/`Away` carry the departed day's `DayEvent`
    /// plan, so a midnight-crossing trip completes and arrives on its own
    /// schedule instead of freezing when the next day rolls non-driving.
    /// Blobs written by v1 builds (payload-less phases) cannot decode into
    /// the new shape — the version gate rejects them here, at the checkpoint
    /// boundary, instead of postcard failing inside `load_state`.
    fn checkpoint_version(&self) -> u32 {
        2
    }

    fn rng_pair(&self) -> Option<([u8; 32], u64)> {
        Some((self.rng.get_seed(), self.rng.get_stream()))
    }
}

fn phase_as_f64(phase: DriverPhase) -> f64 {
    match phase {
        DriverPhase::HomePluggedIn => 0.0,
        DriverPhase::Driving { .. } => 1.0,
        DriverPhase::Away { .. } => 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::preference::PreferenceVote;
    use super::*;
    use crate::actor::testing::test_env;
    use hares_types::{
        CoreOutput, CoreState, ElectricPower, ElectricalSummary, EquipmentId, PriceSignal, Soc,
    };

    fn seed_from_u64(seed: u64) -> ChaCha8Rng {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&seed.to_le_bytes());
        ChaCha8Rng::from_seed(bytes)
    }

    fn make_actor(strategy: ChargingStrategy, policy: PlugInPolicy, seed: u64) -> EvDriverActor {
        EvDriverActor::new(
            "TestDriver",
            "EV1",
            strategy,
            policy,
            ScheduleSource::Constant(30.0),  // 30 miles/day
            ScheduleSource::Constant(480.0), // depart 08:00
            ScheduleSource::Constant(600.0), // 10h away → arrive 18:00
            None,                            // no direct arrival sampling
            1.0,                             // event every day
            0.3,                             // 0.3 kWh/mi
            60.0,                            // 60 kWh battery
            7.2,                             // L2 charge rate
            30.0,                            // 30 mph average
            20.0,                            // 20 miles range anxiety buffer
            0.0,                             // no away charging
            6.6,                             // workplace L2 default
            seed_from_u64(seed),
        )
    }

    /// Drive the actor through departure, driving, arrival, then one more
    /// HomePluggedIn step. Returns the dispatch from the post-arrival step.
    fn drive_cycle_and_charge_step(actor: &mut EvDriverActor) -> Vec<DispatchRequest> {
        let mut out = Vec::new();

        // Depart at 08:00
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();

        // Drive through all steps
        for step in 1..=100 {
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
            out.clear();
        }

        // Arrive at 18:00 -- plug in, transition to HomePluggedIn
        actor.decide(&env_at_minute(18 * 60), &mut out);
        out.clear();

        // Next step at 18:01 -- now in HomePluggedIn, composer evaluates
        actor.decide(&env_at_minute(18 * 60 + 1), &mut out);
        out
    }

    /// Build an actor already in HomePluggedIn phase with specific SOC.
    fn make_plugged_in_actor(strategy: ChargingStrategy, soc: f64) -> EvDriverActor {
        let mut actor = make_actor(strategy, PlugInPolicy::Always, 42);
        actor.estimated_soc = soc;
        actor.phase = DriverPhase::HomePluggedIn;
        // Roll an event so we have departure_minute for context
        let mut out = Vec::new();
        actor.decide(&env_at_minute(0), &mut out);
        actor.phase = DriverPhase::HomePluggedIn;
        actor.estimated_soc = soc;
        actor
    }

    fn plugged_in_step(actor: &mut EvDriverActor, minute: u16) -> Vec<DispatchRequest> {
        let mut out = Vec::new();
        actor.decide(&env_at_minute(minute), &mut out);
        out
    }

    fn plugged_in_step_with_env(
        actor: &mut EvDriverActor,
        env: &EnvironmentState,
    ) -> Vec<DispatchRequest> {
        let mut out = Vec::new();
        actor.decide(env, &mut out);
        out
    }

    /// A dispatch commands charging iff it asserts a positive target or
    /// rate. `PowerSetpoint{active_power_kw: 0.0}` is the explicit hold an
    /// idling strategy dispatches in place of silence — the opposite of a
    /// charging command, so idle-assertions must not count it as one.
    fn commands_charging(req: &DispatchRequest) -> bool {
        match &req.signal {
            ControlSignal::SOCTarget { .. } | ControlSignal::EvSetReadyBy { .. } => true,
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => *active_power_kw > 1e-9,
            _ => false,
        }
    }

    /// The explicit zero-power hold an idling strategy dispatches instead
    /// of going silent (see the module's dispatch-contract doc).
    fn is_explicit_hold(req: &DispatchRequest) -> bool {
        matches!(
            &req.signal,
            ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw.abs() <= 1e-9
        )
    }

    fn env_at_minute(minute: u16) -> EnvironmentState {
        env_at_minute_temp(minute, 10.0)
    }

    /// Test env at an exact minute-of-day with a chosen outdoor temperature.
    /// `env_at_minute` pins the 10 °C default; the anxiety tests need the
    /// 22 °C efficiency baseline at sub-hour minutes.
    fn env_at_minute_temp(minute: u16, outdoor_temp_c: f64) -> EnvironmentState {
        env_at_day_minute_temp(1, minute, outdoor_temp_c)
    }

    fn set_core_soc(
        actor: &mut EvDriverActor,
        env: &mut EnvironmentState,
        equipment_name: &str,
        equipment_id: EquipmentId,
        soc: f64,
    ) {
        let mut by_name = std::collections::HashMap::new();
        by_name.insert(equipment_name.to_string(), equipment_id);
        actor.resolve_equipment_id(&by_name);
        env.equipment_core.insert(
            equipment_id,
            CoreOutput {
                state: CoreState {
                    soc: Some(Soc::try_from(soc).expect("valid test soc")),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
    }

    /// Mirror the EV equipment's committed `CoreOutput` while charging: the
    /// equipment publishes both `state.soc` and `flows.electric_kw` from its
    /// `step()` (hares-equipment/src/ev/mod.rs, CoreOutput construction), so
    /// a test environment modelling "equipment actively charging" supplies
    /// both observations.
    fn set_core_charging(
        actor: &mut EvDriverActor,
        env: &mut EnvironmentState,
        equipment_name: &str,
        equipment_id: EquipmentId,
        soc: f64,
        charge_kw: f64,
    ) {
        set_core_soc(actor, env, equipment_name, equipment_id, soc);
        if let Some(co) = env.equipment_core.get_mut(&equipment_id) {
            co.flows.electric_kw = Some(ElectricPower::Bidirectional(charge_kw));
        }
    }

    #[test]
    fn immediate_strategy_emits_soc_target_on_plugged_in_step() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let mut out = Vec::new();

        // Departure
        let env_depart = env_at_minute(8 * 60);
        actor.decide(&env_depart, &mut out);

        let has_disconnect = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::Disconnected
                }
            )
        });
        assert!(has_disconnect, "expected disconnect on departure");

        // Drive through steps
        out.clear();
        for step in 1..=100 {
            let env = env_at_minute(8 * 60 + step);
            actor.decide(&env, &mut out);
        }

        // Arrive at 18:00
        out.clear();
        let env_arrive = env_at_minute(18 * 60);
        actor.decide(&env_arrive, &mut out);

        let has_plug_in = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        assert!(has_plug_in, "expected plug-in on arrival");

        // Next step: composer evaluates per-step
        out.clear();
        actor.decide(&env_at_minute(18 * 60 + 1), &mut out);

        let has_soc_target = out.iter().any(|r| {
            matches!(r.signal, ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 0.01)
        });
        assert!(
            has_soc_target,
            "expected SOCTarget with target_soc=0.9 from composer, got: {:?}",
            out
        );
    }

    #[test]
    fn perceived_soc_returns_estimated_soc_not_equipment_core() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.estimated_soc = 0.45;
        let mut env = env_at_minute(12 * 60);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.73);
        assert!((actor.perceived_soc() - 0.45).abs() < 1e-12);
    }

    #[test]
    fn actual_soc_reads_from_equipment_core() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let mut env = env_at_minute(12 * 60);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.73);
        assert_eq!(actor.actual_soc(&env), Some(0.73));
    }

    #[test]
    fn actual_soc_returns_none_when_equipment_id_not_resolved() {
        let actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let mut env = env_at_minute(12 * 60);
        env.equipment_core.insert(
            EquipmentId(99),
            CoreOutput {
                state: CoreState {
                    soc: Some(Soc::try_from(0.9).expect("valid test soc")),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert_eq!(actor.actual_soc(&env), None);
    }

    #[test]
    fn perceived_soc_ignores_equipment_core_telemetry() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.estimated_soc = 0.52;
        let mut env = env_at_minute(12 * 60);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.91);
        let perceived = actor.perceived_soc();
        let actual = actor.actual_soc(&env);
        assert!((perceived - 0.52).abs() < 1e-12);
        assert_eq!(actual, Some(0.91));
        assert!(
            (perceived - actual.unwrap()).abs() > 0.01,
            "perceived and actual SOC should diverge when estimated differs from equipment"
        );
    }

    /// Invariant: while the vehicle is plugged in at home and the equipment
    /// is actively charging (ground-truth SOC rising every step), the
    /// driver's own telemetry channels must reflect that charging —
    /// regardless of which strategy governs the session. A caller reading
    /// only the driver's channels must never conclude the car is not
    /// charging when it is.
    #[test]
    fn telemetry_reflects_charging_while_equipment_soc_rises() {
        let strategies = [
            ("Immediate", ChargingStrategy::Immediate { target_soc: 0.9 }),
            (
                "Nightly (in off-peak window)",
                ChargingStrategy::Nightly {
                    off_peak_start_hour: 22.0,
                    off_peak_end_hour: 6.0,
                    target_soc: 0.9,
                },
            ),
            (
                "LowSoc (below threshold)",
                ChargingStrategy::LowSoc {
                    threshold: 0.7,
                    target_soc: 0.9,
                },
            ),
            (
                "QuickThenWait (below partial)",
                ChargingStrategy::QuickThenWait { partial_soc: 0.8 },
            ),
        ];

        let mut failures = Vec::new();
        for (label, strategy) in strategies {
            let mut actor = make_plugged_in_actor(strategy, 0.5);

            let mut charge_kw_ever_positive = false;
            let mut soc_ever_rose = false;
            let mut needed_charge_hours_ever_positive = false;
            let mut previous_soc = 0.5;

            // 120 one-minute steps from 22:00; the equipment BMS charges the
            // whole time, so the ground-truth core SOC rises on every step and
            // the equipment publishes its charging electric flow (0.001 SOC
            // per minute on a 60 kWh pack = 3.6 kW).
            for step in 0..120u16 {
                let mut env = env_at_minute(22 * 60 + step);
                let equipment_soc = 0.5 + f64::from(step) * 0.001;
                set_core_charging(
                    &mut actor,
                    &mut env,
                    "EV1",
                    EquipmentId(7),
                    equipment_soc,
                    3.6,
                );
                plugged_in_step_with_env(&mut actor, &env);

                let telemetry = actor.telemetry().expect("driver telemetry");
                let charge_kw = telemetry.get("charge_kw").expect("charge_kw key");
                let soc = telemetry.get("soc").expect("soc key");
                let needed = telemetry
                    .get("needed_charge_hours")
                    .expect("needed_charge_hours key");

                charge_kw_ever_positive |= charge_kw > 0.0;
                soc_ever_rose |= soc > previous_soc;
                needed_charge_hours_ever_positive |= needed > 0.0;
                previous_soc = soc;
            }

            if !charge_kw_ever_positive {
                failures.push(format!(
                    "{label}: charge_kw telemetry stayed 0.0 across 120 steps of active charging"
                ));
            }
            if !soc_ever_rose {
                failures.push(format!(
                    "{label}: soc telemetry never rose while equipment SOC climbed 0.5 → 0.619"
                ));
            }
            if !needed_charge_hours_ever_positive {
                failures.push(format!(
                    "{label}: needed_charge_hours telemetry stayed 0.0 while SOC sat 0.4 below target"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "driver telemetry did not reflect charging:\n  {}",
            failures.join("\n  ")
        );
    }

    /// While the vehicle is plugged in at home and the equipment is actively
    /// charging (SOC climbing every step), the `needed_charge_hours` channel
    /// must track the shrinking battery gap, not hold a frozen number. The
    /// fold runs on the observed equipment SOC, so it falls as the session
    /// charges the battery. A regression sourcing the fold from the driver's
    /// *belief* (`perceived_soc()`), which is never updated during a home
    /// charging session, would instead hold the arrival value until the next
    /// day-start reconciliation: a caller reading the driver's channels would
    /// see "still needs N hours" beside a rising `soc` and positive
    /// `charge_kw`.
    #[test]
    fn needed_charge_hours_decreases_while_equipment_charges_at_home() {
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.5);

        let mut env = env_at_minute(22 * 60);
        set_core_charging(&mut actor, &mut env, "EV1", EquipmentId(7), 0.5, 3.6);
        plugged_in_step_with_env(&mut actor, &env);
        let first_needed = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        assert!(
            first_needed > 3.0,
            "precondition: below target at SOC 0.5 the estimate must be positive, got {first_needed}"
        );

        // 120 one-minute steps from 22:01 — the equipment charges the whole
        // time (0.5 → 0.619), while the driver's belief stays 0.5 (no
        // arrival, no day boundary in the window).
        let mut last_needed = first_needed;
        let mut any_charge: f64 = 0.0;
        let mut highest_soc: f64 = 0.5;
        for step in 1..=120u16 {
            let mut env = env_at_minute(22 * 60 + step);
            let equipment_soc = 0.5 + f64::from(step) * 0.001;
            set_core_charging(
                &mut actor,
                &mut env,
                "EV1",
                EquipmentId(7),
                equipment_soc,
                3.6,
            );
            plugged_in_step_with_env(&mut actor, &env);
            let telemetry = actor.telemetry().expect("driver telemetry");
            any_charge = any_charge.max(telemetry.get("charge_kw").expect("charge_kw key"));
            highest_soc = highest_soc.max(telemetry.get("soc").expect("soc key"));
            last_needed = telemetry
                .get("needed_charge_hours")
                .expect("needed_charge_hours key");
        }

        assert!(
            any_charge > 0.0,
            "precondition: the equipment must have charged over the window (charge_kw max = {any_charge})"
        );
        assert!(
            highest_soc > 0.55,
            "precondition: equipment SOC must have climbed over the window (soc max = {highest_soc})"
        );

        assert!(
            last_needed < first_needed - 0.5,
            "needed_charge_hours must fall as the battery charges (SOC 0.5 → 0.619), \
             but it held {last_needed} against the initial {first_needed}; it is frozen at the \
             driver's belief while `soc` and `charge_kw` report ground truth"
        );
    }

    /// Class guard: every `ChargingStrategy` variant's driver telemetry must
    /// reflect active charging while the vehicle is plugged in at home —
    /// `soc` reports the equipment's ground truth, `charge_kw` is positive,
    /// and `needed_charge_hours` is a real estimate (not the collapsed
    /// "no estimate" sentinel) on BOTH dispatch paths: the composer fold
    /// (SOC 0.5, above the range-anxiety threshold) and the range-anxiety
    /// override (SOC 0.2, below it), which bypasses the preference stack.
    /// The reproduction test pins the four strategies the RCA named; this
    /// guard pins the whole enum, so a strategy added later cannot silently
    /// ship channels that read as "never charging".
    #[test]
    fn every_strategy_variant_telemetry_reflects_active_charging() {
        // Compile-time exhaustiveness anchor: adding a `ChargingStrategy`
        // variant breaks this match, forcing the new variant into
        // `all_variants` below until this class guard covers it.
        const _: () = {
            match (ChargingStrategy::Immediate { target_soc: 0.0 }) {
                ChargingStrategy::Immediate { .. }
                | ChargingStrategy::Nightly { .. }
                | ChargingStrategy::LowSoc { .. }
                | ChargingStrategy::QuickThenWait { .. }
                | ChargingStrategy::PreDeparture { .. }
                | ChargingStrategy::TouAware { .. }
                | ChargingStrategy::SolarSurplus { .. }
                | ChargingStrategy::V2H { .. }
                | ChargingStrategy::V2G { .. } => {}
            }
        };

        let all_variants: Vec<(&str, ChargingStrategy)> = vec![
            ("Immediate", ChargingStrategy::Immediate { target_soc: 0.9 }),
            (
                "Nightly",
                ChargingStrategy::Nightly {
                    off_peak_start_hour: 22.0,
                    off_peak_end_hour: 6.0,
                    target_soc: 0.9,
                },
            ),
            (
                "LowSoc",
                ChargingStrategy::LowSoc {
                    threshold: 0.7,
                    target_soc: 0.9,
                },
            ),
            (
                "QuickThenWait",
                ChargingStrategy::QuickThenWait { partial_soc: 0.8 },
            ),
            (
                "PreDeparture",
                ChargingStrategy::PreDeparture {
                    target_soc: 0.9,
                    departure_schedule: vec![],
                },
            ),
            (
                "TouAware",
                ChargingStrategy::TouAware {
                    target_soc: 0.9,
                    departure_schedule: vec![],
                    charge_buffer_hours: 0.0,
                },
            ),
            (
                "SolarSurplus",
                ChargingStrategy::SolarSurplus {
                    min_charge_rate_kw: 1.0,
                    departure_schedule: vec![],
                },
            ),
            (
                "V2H",
                ChargingStrategy::V2H {
                    discharge_threshold_soc: 0.7,
                    min_soc: 0.2,
                },
            ),
            (
                "V2G",
                ChargingStrategy::V2G {
                    min_soc: 0.3,
                    max_export_kw: 5.0,
                    price_threshold: 0.20,
                },
            ),
        ];

        let mut failures = Vec::new();
        for (label, strategy) in all_variants {
            // TouAware is the only variant whose preference stack needs a
            // price schedule at construction.
            let needs_price_schedule = matches!(strategy, ChargingStrategy::TouAware { .. });

            // Two SOC levels exercise both dispatch paths: 0.5 sits above
            // the range-anxiety threshold (~0.28 for this actor) so the
            // composer fold runs; 0.2 sits below it so the range-anxiety
            // override bypasses the preference stack.
            for starting_soc in [0.5, 0.2] {
                let mut actor = make_plugged_in_actor(strategy.clone(), starting_soc);
                if needs_price_schedule {
                    actor = actor.with_price_schedule(vec![0.10; 24].into(), 24);
                }

                // 0.5: one step at 22:00 (inside Nightly's off-peak window)
                // with the equipment actively charging — the composer path.
                // 0.2: one step at 07:45, 25 minutes before the 08:00
                // departure — inside the anxiety band AND past the
                // override's urgency gate (needed to clear the band ≈ 0.8 h,
                // ×1.2 margin ≈ 0.96 h ≫ 25 min), so the override fires for
                // every variant. At 22:00 the same SOC would leave ~10 h
                // before the next departure and the gate would stand the
                // override down.
                let mut env = if starting_soc > 0.3 {
                    env_at_minute(22 * 60)
                } else {
                    env_at_minute(7 * 60 + 45)
                };
                if needs_price_schedule {
                    env.price_signal = PriceSignal {
                        electricity_price: Some(0.02),
                        ..Default::default()
                    };
                }
                // SolarSurplus's composer path votes a charge rate only
                // when PV surplus exists; without surplus its correct
                // decision is a hold, which would (rightly) report
                // charge_kw = 0. Supply the surplus the strategy votes on.
                if matches!(strategy, ChargingStrategy::SolarSurplus { .. }) {
                    env.electrical = ElectricalSummary {
                        pv_generation_kw: 3.0,
                        base_load_kw: 1.0,
                        ..Default::default()
                    };
                }
                let equipment_soc = (starting_soc + 0.01).min(1.0);
                set_core_charging(
                    &mut actor,
                    &mut env,
                    "EV1",
                    EquipmentId(7),
                    equipment_soc,
                    3.6,
                );
                plugged_in_step_with_env(&mut actor, &env);

                let telemetry = actor.telemetry().expect("driver telemetry");
                let soc = telemetry.get("soc").expect("soc key");
                let charge_kw = telemetry.get("charge_kw").expect("charge_kw key");
                let needed = telemetry
                    .get("needed_charge_hours")
                    .expect("needed_charge_hours key");

                if (soc - equipment_soc).abs() > 1e-9 {
                    failures.push(format!(
                        "{label} @ SOC {starting_soc}: soc telemetry ({soc}) must report the equipment's ground-truth SOC ({equipment_soc})"
                    ));
                }
                if charge_kw <= 0.0 {
                    failures.push(format!(
                        "{label} @ SOC {starting_soc}: charge_kw telemetry ({charge_kw}) must be positive while the equipment charges"
                    ));
                }
                if needed <= 0.0 {
                    failures.push(format!(
                        "{label} @ SOC {starting_soc}: needed_charge_hours telemetry ({needed}) must be a real estimate below target, not the collapsed no-estimate sentinel"
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "strategy variants whose telemetry does not reflect active charging:\n  {}",
            failures.join("\n  ")
        );
    }

    /// Non-driving days route through `evaluate_charging` like driving
    /// days; the `needed_charge_hours` channel must still report a real
    /// estimate — the standing strategy plan's while resting above the
    /// anxiety threshold, the override's (anxiety-band) while anxious.
    #[test]
    fn needed_charge_hours_on_non_driving_day_reports_active_plan() {
        // Above the anxiety threshold (~0.28): the strategy plan stands and
        // its decision dispatches. `make_plugged_in_actor` rolled a driving
        // day at minute 0; clearing the event models the non-driving-day
        // branch of `decide`.
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.5);
        actor.event_day_ratio = 0.0;
        actor.todays_event = None;
        let mut env = env_at_minute(12 * 60);
        set_core_charging(&mut actor, &mut env, "EV1", EquipmentId(7), 0.51, 3.6);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 1e-9
            )),
            "a resting non-driving day must dispatch the standing strategy plan (SOCTarget 0.9), got {out:?}"
        );
        let needed = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        assert!(
            needed > 0.0,
            "needed_charge_hours on a resting non-driving day must report the standing plan's estimate, got {needed}"
        );

        // Below the anxiety threshold: the override fires and its own
        // target (the anxiety band) drives the estimate. The strategy is a
        // Nightly window (22:00–06:00) so the 12:00 step sits outside it:
        // the composer resolves no target, and the range-anxiety override —
        // which under the precedence rule only pre-empts when no active
        // strategy target already covers the band — governs. (With an
        // in-window Immediate 0.9 the strategy's own target would stand the
        // override down by design: pre-empting 0.9 with the band charges
        // less than configured for zero safety gain.)
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            0.15,
        );
        actor.event_day_ratio = 0.0;
        actor.todays_event = None;
        let mut env = env_at_minute(12 * 60);
        set_core_charging(&mut actor, &mut env, "EV1", EquipmentId(7), 0.16, 3.6);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. }
                    if (target_soc - anxiety_band_non_driving_day_at_10c()).abs() < 1e-9
            )),
            "anxious non-driving day must dispatch the minimal SOCTarget(band)"
        );
        let needed = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        assert!(
            needed > 0.0,
            "needed_charge_hours on an anxious non-driving day must report the override's estimate, got {needed}"
        );

        // First-ever step is a non-driving day: a fresh actor that has never
        // run a plugged-in evaluation still has a composer whose estimate
        // slot holds the "no estimate" sentinel — the step-start refresh
        // must replace it before the channel is published.
        let mut fresh = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        fresh.event_day_ratio = 0.0;
        fresh.estimated_soc = 0.5;
        fresh.phase = DriverPhase::HomePluggedIn;
        let mut env = env_at_minute(12 * 60);
        set_core_charging(&mut fresh, &mut env, "EV1", EquipmentId(7), 0.51, 3.6);
        let mut out = Vec::new();
        fresh.decide(&env, &mut out);
        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 1e-9
            )),
            "a resting non-driving day as the first-ever step must dispatch the strategy decision, got {out:?}"
        );
        let needed = fresh
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        assert!(
            needed > 0.0,
            "needed_charge_hours on the first-ever step (non-driving day, no prior evaluation) must report a real estimate, not the fresh composer's no-estimate sentinel, got {needed}"
        );
    }

    /// `charge_kw` under goal-based control (no rate dispatched) must report
    /// the equipment's observed charging power — the exact value, not merely
    /// a positive one — and must return to 0.0 when the equipment stops
    /// charging, so the channel can never report phantom charging.
    #[test]
    fn charge_kw_tracks_observed_equipment_flow_under_goal_control() {
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.5);

        // Equipment charging at 3.6 kW (SOC rising 0.001/step on 60 kWh).
        let mut env = env_at_minute(22 * 60);
        set_core_charging(&mut actor, &mut env, "EV1", EquipmentId(7), 0.501, 3.6);
        plugged_in_step_with_env(&mut actor, &env);
        let charge_kw = actor
            .telemetry()
            .expect("driver telemetry")
            .get("charge_kw");
        assert_eq!(
            charge_kw,
            Some(3.6),
            "charge_kw must report the observed equipment flow (3.6 kW) when no rate is dispatched"
        );

        // Equipment idle (SOC flat, zero published flow) — no phantom charging.
        let mut env = env_at_minute(22 * 60 + 1);
        set_core_charging(&mut actor, &mut env, "EV1", EquipmentId(7), 0.501, 0.0);
        plugged_in_step_with_env(&mut actor, &env);
        let charge_kw = actor
            .telemetry()
            .expect("driver telemetry")
            .get("charge_kw");
        assert_eq!(
            charge_kw,
            Some(0.0),
            "charge_kw must be 0.0 when the equipment publishes zero flow"
        );
    }

    /// A rate dispatched this step is the driver's own command and wins over
    /// the observed (previous-step) equipment flow.
    #[test]
    fn charge_kw_prefers_dispatched_rate_over_observed_flow() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2G {
                min_soc: 0.3,
                max_export_kw: 5.0,
                price_threshold: 0.20,
            },
            0.7,
        );
        let mut env = env_at_minute(19 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.30),
            ..Default::default()
        };
        // Previous-step equipment flow says +3.6 (charging); this step the
        // driver commands a -5.0 kW export.
        set_core_charging(&mut actor, &mut env, "EV1", EquipmentId(7), 0.71, 3.6);
        plugged_in_step_with_env(&mut actor, &env);
        let charge_kw = actor
            .telemetry()
            .expect("driver telemetry")
            .get("charge_kw");
        assert_eq!(
            charge_kw,
            Some(-5.0),
            "dispatched rate (-5.0) must take precedence over the observed previous-step flow (3.6)"
        );
    }

    /// The `soc` channel reports the equipment's ground-truth SOC whenever
    /// the equipment is observable — without disturbing the behavioral
    /// estimate, which keeps its documented divergence.
    #[test]
    fn soc_telemetry_reports_equipment_ground_truth_while_observable() {
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.5);
        let mut env = env_at_minute(22 * 60);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.77);
        plugged_in_step_with_env(&mut actor, &env);

        let soc = actor.telemetry().expect("driver telemetry").get("soc");
        assert_eq!(
            soc,
            Some(0.77),
            "soc telemetry must report the equipment's ground-truth SOC while observable"
        );
        assert_eq!(
            actor.actual_soc(&env),
            Some(0.77),
            "equipment ground truth should still read 0.77"
        );
        assert!(
            (actor.perceived_soc() - 0.5).abs() < 1e-12,
            "the behavioral estimate must be unchanged by telemetry reporting"
        );
    }

    /// When the equipment is not observable (unresolved equipment id), the
    /// `soc` channel degrades to the driver's estimate — the documented
    /// fallback for the warned no-feedback mode.
    #[test]
    fn soc_telemetry_falls_back_to_estimate_when_equipment_unobservable() {
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.5);
        // No resolve_equipment_id call and no equipment_core entry.
        let env = env_at_minute(22 * 60);
        plugged_in_step_with_env(&mut actor, &env);
        let soc = actor.telemetry().expect("driver telemetry").get("soc");
        assert_eq!(
            soc,
            Some(0.5),
            "soc telemetry must fall back to the driver's estimate when the equipment is unobservable"
        );
    }

    /// Away charging persists in equipment state long after the single
    /// `EvAwayCharge` dispatch; on Away steps with no dispatch to scan,
    /// `charge_kw` must read the equipment's away-charge telemetry, and must
    /// fall back to 0.0 when that observation is absent.
    #[test]
    fn charge_kw_reports_away_charging_on_non_dispatch_steps() {
        let mut actor = make_away_charge_actor(42);
        let mut out = Vec::new();

        // Depart at 08:00 and drive until the trip completes.
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();
        for step in 1..=100 {
            out.clear();
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
            if matches!(actor.phase, DriverPhase::Away { .. }) {
                break;
            }
        }

        // First Away step emits the deferred AwayPluggedIn + EvAwayCharge.
        out.clear();
        actor.decide(&env_at_minute(8 * 60 + 101), &mut out);
        assert!(
            out.iter()
                .any(|r| matches!(r.signal, ControlSignal::EvAwayCharge { .. })),
            "expected the deferred EvAwayCharge dispatch on the first Away step"
        );

        // A later Away step dispatches nothing; the equipment's away-charge
        // telemetry (12:00, well before the 18:00 arrival) is the only
        // observable. 12:00 is minute 720 — past the drive, before arrival.
        let mut env = env_at_minute(12 * 60);
        let mut away_telemetry = Telemetry::new();
        away_telemetry.insert(tk::AWAY_CHARGE_POWER_KW, 6.6);
        env.equipment_telemetry
            .insert("EV1".to_string(), away_telemetry);
        out.clear();
        actor.decide(&env, &mut out);
        assert!(
            out.is_empty(),
            "a mid-away step should dispatch nothing, got {out:?}"
        );
        let charge_kw = actor
            .telemetry()
            .expect("driver telemetry")
            .get("charge_kw");
        assert_eq!(
            charge_kw,
            Some(6.6),
            "charge_kw must report the equipment's away-charge power on non-dispatch Away steps"
        );

        // Without the equipment telemetry observation the channel degrades
        // to 0.0 rather than inventing a value.
        let env = env_at_minute(12 * 60 + 1);
        out.clear();
        actor.decide(&env, &mut out);
        let charge_kw = actor
            .telemetry()
            .expect("driver telemetry")
            .get("charge_kw");
        assert_eq!(
            charge_kw,
            Some(0.0),
            "charge_kw must degrade to 0.0 when the away-charge observation is absent"
        );
    }

    /// While driving, the vehicle is off the charger: `charge_kw` must be
    /// 0.0 on steps that dispatch `EvDrive` (driving energy is not charger
    /// power), and the Driving phase must never attribute an observable
    /// equipment flow to the charge channel.
    #[test]
    fn charge_kw_is_zero_while_driving() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let mut out = Vec::new();

        // Depart at 08:00.
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();

        // First driving step dispatches EvDrive. Even with an equipment flow
        // still observable in the core, the Driving phase reads no charger
        // power.
        let mut env = env_at_minute(8 * 60 + 1);
        set_core_charging(&mut actor, &mut env, "EV1", EquipmentId(7), 0.49, 3.6);
        actor.decide(&env, &mut out);
        assert!(
            out.iter()
                .any(|r| matches!(r.signal, ControlSignal::EvDrive { .. })),
            "expected an EvDrive dispatch on the first driving step, got {out:?}"
        );
        let charge_kw = actor
            .telemetry()
            .expect("driver telemetry")
            .get("charge_kw");
        assert_eq!(
            charge_kw,
            Some(0.0),
            "charge_kw must be 0.0 while driving — the vehicle is off the charger"
        );
    }

    /// The observed equipment flow is signed: a published export (negative
    /// kW) with no rate dispatched this step must be reported as negative,
    /// not clamped to zero — the channel stays truthful about the direction
    /// of power at the charger.
    #[test]
    fn charge_kw_reports_negative_observed_export_flow() {
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.5);
        let mut env = env_at_minute(22 * 60);
        set_core_charging(&mut actor, &mut env, "EV1", EquipmentId(7), 0.499, -4.0);
        plugged_in_step_with_env(&mut actor, &env);
        let charge_kw = actor
            .telemetry()
            .expect("driver telemetry")
            .get("charge_kw");
        assert_eq!(
            charge_kw,
            Some(-4.0),
            "charge_kw must report the signed observed flow (-4.0 export), not clamp to 0.0"
        );
    }

    /// The `needed_charge_hours` channel is published on every step in every
    /// phase and promises an estimate from the *current* battery state.
    /// While the car drives, the battery drains and the charge time back to
    /// target must grow accordingly — a value frozen at the last plugged-in
    /// step reads as a live estimate that no longer reflects the battery,
    /// the same "telemetry that does not reflect what is happening" failure
    /// the channel exists to prevent, one phase over.
    #[test]
    fn needed_charge_hours_grows_as_soc_drains_while_driving() {
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.5);

        // Last plugged-in step before the 08:00 departure: the composer
        // refreshes the estimate from SOC 0.5.
        plugged_in_step(&mut actor, 7 * 60 + 59);
        let needed_before = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        assert!(
            needed_before > 0.0,
            "plugged in at SOC 0.5 with target 0.9, the estimate must be positive, got {needed_before}"
        );

        // Depart at 08:00.
        let mut out = Vec::new();
        actor.decide(&env_at_minute(8 * 60), &mut out);
        assert!(
            matches!(actor.phase, DriverPhase::Driving { .. }),
            "expected the actor to be driving after the 08:00 departure"
        );

        // Drive for 30 one-minute steps; the equipment's ground-truth SOC
        // drains as the trip's energy is spent (≈0.166 SOC over the 67-step
        // trip at 10°C).
        let soc_before = actor.perceived_soc();
        for step in 1..=30u16 {
            let mut env = env_at_minute(8 * 60 + step);
            let equipment_soc = 0.5 - 0.166 * (f64::from(step) / 67.0);
            set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), equipment_soc);
            out.clear();
            actor.decide(&env, &mut out);
        }
        let soc_after = actor.perceived_soc();
        assert!(
            soc_after < soc_before - 0.05,
            "the trip must drain the battery meaningfully for this test to bite: {soc_before} → {soc_after}"
        );

        let needed_after = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        assert!(
            needed_after > needed_before,
            "needed_charge_hours must grow as the battery drains (SOC {soc_before:.3} → {soc_after:.3}), \
             but the channel held {needed_after} against the pre-departure {needed_before}"
        );
    }

    /// Checkpoint/restore reconstructs the actor with a fresh composer whose
    /// estimate slot starts at the "no estimate" sentinel. On a Driving step
    /// after a mid-trip restore, no plugged-in path runs to refresh it — so
    /// the published channel must still report a real estimate from the
    /// restored SOC, not the sentinel collapse to 0.0 while the battery sits
    /// far below target.
    #[test]
    fn needed_charge_hours_survives_checkpoint_restore_while_driving() {
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.5);

        // Refresh the estimate on the last plugged-in step, then depart and
        // drive a few steps so the checkpoint captures a mid-trip Driving
        // state with the battery below the plugged-in SOC.
        plugged_in_step(&mut actor, 7 * 60 + 59);
        let mut out = Vec::new();
        actor.decide(&env_at_minute(8 * 60), &mut out);
        for step in 1..=5u16 {
            out.clear();
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
        }
        assert!(
            matches!(actor.phase, DriverPhase::Driving { .. }),
            "checkpoint must capture a mid-trip Driving state"
        );
        let restored_soc = actor.perceived_soc();
        let blob = actor.save_state().expect("save_state should succeed");

        // Restore into a freshly-constructed actor — the production
        // checkpoint-restore path — and continue the trip.
        let mut restored = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        restored
            .load_state(&blob)
            .expect("load_state should succeed");
        let mut env = env_at_minute(8 * 60 + 6);
        set_core_soc(&mut restored, &mut env, "EV1", EquipmentId(7), restored_soc);
        out.clear();
        restored.decide(&env, &mut out);

        let needed = restored
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        assert!(
            needed > 0.0,
            "after a mid-trip restore at SOC {restored_soc:.3} (target 0.9), needed_charge_hours \
             must report a real estimate, not the collapsed no-estimate sentinel (0.0)"
        );
    }

    /// Mirror `make_actor` with a configurable driving-day probability, so
    /// tests can force a non-driving day deterministically.
    fn make_actor_with_event_ratio(
        strategy: ChargingStrategy,
        event_day_ratio: f64,
        seed: u64,
    ) -> EvDriverActor {
        EvDriverActor::new(
            "TestDriver",
            "EV1",
            strategy,
            PlugInPolicy::Always,
            ScheduleSource::Constant(30.0),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(600.0),
            None,
            event_day_ratio,
            0.3,
            60.0,
            7.2,
            30.0,
            20.0,
            0.0,
            6.6,
            seed_from_u64(seed),
        )
    }

    /// Charge-time estimate for an arbitrary SOC gap on a 60 kWh pack at
    /// the 22°C efficiency baseline (temp multiplier 1.0), rounded up to
    /// whole one-minute steps — the same expectation style as the
    /// soc_target needed-charge-hours tests.
    fn expected_hours_at_epa_baseline(soc_gap: f64) -> f64 {
        let raw: f64 = soc_gap * 60.0 / (7.2 * 0.9);
        let step_hours: f64 = 1.0 / 60.0;
        (raw / step_hours).ceil() * step_hours
    }

    /// The anxiety band for the standard `make_actor` fixture on a DRIVING
    /// day at the 10 °C default: the rolled trip is 30 mi × 1.11 = 33.3
    /// day-specific miles (blended above the 30 mi expectation), so the band
    /// is (33.3 + 20) × 0.3 × 1.11 / 60 ≈ 0.2958 SOC — the minimal charge
    /// target (trip + safe reserve).
    fn anxiety_band_driving_day_at_10c() -> f64 {
        ((30.0 * 1.11) + 20.0) * 0.3 * 1.11 / 60.0
    }

    /// The anxiety band on a NON-driving day at the 10 °C default: no rolled
    /// trip, so the band uses the 30 mi expectation — (30 + 20) × 0.3 ×
    /// 1.11 / 60 = 0.2775 SOC.
    fn anxiety_band_non_driving_day_at_10c() -> f64 {
        (30.0 + 20.0) * 0.3 * 1.11 / 60.0
    }

    /// While the range-anxiety override is active, the plan actually
    /// dispatched is the minimal charge to the band (`SOCTarget(band)` —
    /// trip + safe reserve, not full) — so the `needed_charge_hours` channel
    /// must report the to-band estimate, not the standing strategy plan's
    /// (here, target 0.9). If the override path ever stops substituting its
    /// own estimate, the channel keeps publishing the standing plan's
    /// number while the equipment charges a different plan.
    #[test]
    fn needed_charge_hours_under_anxiety_reports_minimal_band_plan() {
        // Nightly window (22:00–06:00): the 07:45 step is outside it, so the
        // composer resolves no target and the override governs — under the
        // precedence rule an in-window strategy target ≥ band (e.g.
        // Immediate 0.9) would stand the override down by design.
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            0.2,
        );

        // 07:45 on a driving day (departure 08:00) at the 22 °C efficiency
        // baseline; SOC 0.2 sits below the anxiety band and time to
        // departure is short enough that the override's urgency gate holds,
        // so the override fires inside evaluate_charging. The band blends
        // the day's rolled trip (30 mi × 1.11 at the 10 °C roll = 33.3 mi)
        // with the buffer: (33.3 + 20) × 0.3 / 60 = 0.2665.
        //   needed (0.2 → 0.2665) = 3.99 kWh / (7.2 × 0.9) ≈ 0.616 h
        //   urgency threshold   = 0.616 × 1.2 ≈ 0.74 h (44.3 min)
        //   time left at 07:45  = 25 min < 44.3 min → override fires.
        // (At 22:00 the same SOC leaves ~10 h before the next departure —
        // the gate stands the override down and the strategy governs.)
        let mut env = env_at_minute_temp(7 * 60 + 45, 22.0);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.2);
        let out = plugged_in_step_with_env(&mut actor, &env);

        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.2665).abs() < 1e-9
            )),
            "anxiety must dispatch the minimal SOCTarget(0.2665 — trip + reserve), not a charge \
             to full, got {out:?}"
        );

        let needed = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        let expected = expected_hours_at_epa_baseline(0.0665); // 0.2 → band 0.2665
        assert!(
            (needed - expected).abs() < 1e-9,
            "under the anxiety override, needed_charge_hours must be the to-band estimate \
             ({expected} h for SOC 0.2 → 0.2665), not the standing plan's (≈4.63 h for 0.2 → 0.9), got {needed}"
        );
    }

    /// The same override-substitution contract on the non-driving-day arm,
    /// where the override's early return inside `evaluate_charging` bypasses
    /// the composer: the published
    /// estimate must be the override's to-band figure, not the
    /// standing strategy plan's fold refreshed at step start.
    #[test]
    fn needed_charge_hours_under_anxiety_on_non_driving_day_reports_minimal_band_plan() {
        let mut actor = make_actor_with_event_ratio(
            // Nightly window (22:00–06:00): the 12:00 step is outside it, so
            // the composer resolves no target and the override governs —
            // under the precedence rule an in-window strategy target ≥ band
            // would stand the override down by design.
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            0.0, // never a driving day
            42,
        );
        actor.estimated_soc = 0.2;
        actor.phase = DriverPhase::HomePluggedIn;

        // 12:00 at the 22°C efficiency baseline; SOC 0.2 sits below the
        // non-driving-day anxiety band: no rolled trip, so the band uses
        // the 30 mi expectation — (30 + 20) × 0.3 / 60 = 0.25.
        let mut env = test_env().hour(12).outdoor_temp(22.0).build();
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.2);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.25).abs() < 1e-9
            )),
            "anxiety on a non-driving day must dispatch the minimal SOCTarget(0.25), got {out:?}"
        );

        let needed = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        let expected = expected_hours_at_epa_baseline(0.05); // 0.2 → band 0.25
        assert!(
            (needed - expected).abs() < 1e-9,
            "under the anxiety override on a non-driving day, needed_charge_hours must be the \
             to-band estimate ({expected} h for SOC 0.2 → 0.25), not the standing plan's \
             (≈4.63 h for 0.2 → 0.9), got {needed}"
        );
    }

    /// The `needed_charge_hours` channel reads the *observed* battery gap
    /// every step — so during an overnight charging session it must already
    /// reflect the charged SOC at 23:59, before any arrival or day-start
    /// reconciliation refreshes the driver's belief. A sourcing regression
    /// back to `perceived_soc()` would publish the stale belief's hours
    /// (≈4.11 h from 0.5 to 0.9) beside `soc` = 0.9 and `charge_kw` = 0 —
    /// "still needs four hours" on a full, idle battery. The rollover step
    /// then pins that the day-start observation and belief reconciliation
    /// do not disturb the channel.
    #[test]
    fn needed_charge_hours_on_day_rollover_reflects_observed_charged_soc() {
        let mut actor = make_actor_with_event_ratio(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            0.0, // never a driving day — stays HomePluggedIn across midnight
            42,
        );
        actor.estimated_soc = 0.5;
        actor.phase = DriverPhase::HomePluggedIn;

        // 12:00 on day 1: the first step rolls day 1 and its day-start
        // observation reconciles the belief — the equipment also reads 0.5,
        // so the belief stays 0.5 consistently.
        let mut env = test_env().hour(12).outdoor_temp(10.0).build();
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.5);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        // 23:59 on day 1 (same ordinal — no reconciliation): the equipment
        // has charged to the 0.9 target overnight, but the driver's belief
        // is still the stale 0.5 — the channel must report the observed
        // gap (≈0 h), not the belief's ≈4.11 h.
        let mut env = env_at_minute(23 * 60 + 59);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.9);
        out.clear();
        actor.decide(&env, &mut out);
        assert!(
            actor.perceived_soc() < 0.6,
            "precondition: the driver's belief must still be the stale 0.5 at 23:59 — \
             otherwise the ≈0 estimate below could come from the belief, not the observation \
             (got {})",
            actor.perceived_soc()
        );
        let needed_before_rollover = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        assert!(
            needed_before_rollover.abs() < 1e-9,
            "before any reconciliation the estimate must reflect the observed 0.9 SOC (≈0 h \
             needed, battery at target), not the stale 0.5 belief (≈4.11 h), got \
             {needed_before_rollover}"
        );

        // 00:00 on day 2: the day-start observation reconciles the belief to
        // the equipment's 0.9 — the channel must be unaffected (still ≈0).
        let mut env = test_env().hour(0).date(2026, 1, 2).build();
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.9);
        out.clear();
        actor.decide(&env, &mut out);
        let needed_after_rollover = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        assert!(
            needed_after_rollover.abs() < 1e-9,
            "the rollover step must keep reporting the observed at-target gap (≈0 h), got \
             {needed_after_rollover}"
        );
    }

    /// The needed-hours estimate keys on the equipment's own published
    /// cold-charge capability (`CHARGE_DERATE` telemetry) when observable —
    /// one source of truth with the model that actually moves the power —
    /// not the ambient driving-range curve. A pack derated to half power
    /// doubles the estimate.
    #[test]
    fn needed_charge_hours_keys_on_equipment_charge_derate() {
        let mut actor = make_actor_with_event_ratio(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            0.0, // never a driving day — stays HomePluggedIn
            42,
        );
        actor.estimated_soc = 0.5;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut env = env_at_minute(22 * 60);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.5);
        let mut t = Telemetry::new();
        t.insert(tk::CHARGE_DERATE, 0.5);
        env.equipment_telemetry.insert("EV1".to_string(), t);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        let needed = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        // 0.5 → 0.9 gap = 24 kWh at 7.2 kW × η 0.9 × derate 0.5 → 7.41 h
        // (the minute-resolution ceiling absorbs the rounding).
        let expected = 0.4 * 60.0 / (7.2 * 0.9 * 0.5);
        assert!(
            (needed - expected).abs() < 0.02,
            "with the equipment's derate at 0.5 the estimate must double the \
             full-capability hours ({expected:.3} h), got {needed:.3} — the ambient \
             fallback curve is being used instead of the observed capability"
        );
    }

    /// With charging physically impossible right now (the equipment's
    /// published derate is 0 — pack at/below the plating cutoff,
    /// preconditioning in progress), the estimate itself must read +∞ —
    /// impossible, never "slower" — while the telemetry channel publishes
    /// the finite ambient-curve fallback (its finiteness contract), with
    /// the blocked state carried exactly by the equipment's own
    /// `CHARGE_DERATE` column. An estimate keyed only on the ambient curve
    /// caps at ~2× while the equipment correctly zeroes charge power below
    /// the cutoff — the needed-hours channel then climbs nightly while
    /// nothing charges, the exact diagnostic signature this design removes.
    #[test]
    fn needed_charge_hours_reports_impossible_when_charge_is_physically_blocked() {
        let mut actor =
            make_actor_with_event_ratio(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.0, 42);
        actor.estimated_soc = 0.5;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut env = env_at_minute(22 * 60);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.5);
        let mut t = Telemetry::new();
        t.insert(tk::CHARGE_DERATE, 0.0);
        env.equipment_telemetry.insert("EV1".to_string(), t);

        // The estimate itself: +∞ on the observed context (the control
        // consumers — the anxiety gate and the departure deadline — call
        // this directly and read maximum urgency).
        let ctx = DecisionContext {
            current_soc: 0.5,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 7.2,
            env: &env,
            current_minute: 22 * 60,
            next_departure_minute: None,
            time_res_minutes: 1.0,
            observed_charge_derate: Some(0.0),
        };
        let estimate = needed_charge_hours_to_target(0.9, 0.9, &ctx);
        assert!(
            estimate.is_infinite() && estimate.is_sign_positive(),
            "a zero published derate means charging is impossible right now — the \
             estimate must be +∞, got {estimate} (a finite value reports a \
             nonzero capability where there is none)"
        );

        // The telemetry channel: the finite ambient-curve fallback for the
        // same gap (10 °C ambient → multiplier 1.11).
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        let needed = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        let expected = 0.4 * 60.0 / (7.2 * 0.9 / 1.11);
        assert!(
            needed.is_finite() && (needed - expected).abs() < 0.02,
            "the channel must publish the finite ambient fallback ({expected:.3} h) \
             while charging is blocked, got {needed} — the telemetry layer's \
             finiteness contract cannot carry the +∞ control truth"
        );

        // At target, the same blocked pack reads 0 h — nothing is needed,
        // regardless of capability.
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.9);
        out.clear();
        actor.decide(&env, &mut out);
        let needed = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        assert_eq!(
            needed, 0.0,
            "a zero SOC gap needs zero hours even while charging is blocked"
        );
    }

    /// The anxiety override's estimate runs on the observed equipment SOC —
    /// the same telemetry face as the fold it replaces. When the driver's
    /// belief and the equipment's truth diverge (here belief 0.2, truth
    /// 0.6), the channel must report the to-band time from the *observed*
    /// gap — here zero, because the observed pack already sits above the
    /// band and the dispatched minimal charge is a physical no-op — not
    /// from the belief that triggered the anxiety (≈0.47 h to the band):
    /// the equipment charges from its real SOC, so the belief-based figure
    /// describes a session that will not happen.
    #[test]
    fn needed_charge_hours_under_anxiety_reports_observed_gap_not_belief() {
        // Nightly window (22:00–06:00): the 07:45 step is outside it, so the
        // composer resolves no target and the override governs — under the
        // precedence rule an in-window strategy target ≥ band would stand
        // the override down by design.
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            0.2,
        );

        // 07:45 on a driving day (departure 08:00) at the 22 °C baseline;
        // belief 0.2 sits below the anxiety band (0.2665 — the day's rolled
        // trip blends to 33.3 mi) with too little time to close the gap
        // (25 min < 44.3 min urgency threshold — see
        // needed_charge_hours_under_anxiety_reports_minimal_band_plan),
        // but the equipment's truth is 0.6.
        let mut env = env_at_minute_temp(7 * 60 + 45, 22.0);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.6);
        let out = plugged_in_step_with_env(&mut actor, &env);

        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.2665).abs() < 1e-9
            )),
            "anxiety must dispatch the minimal SOCTarget(0.2665) — without the override this test attacks nothing, got {out:?}"
        );
        assert!(
            (actor.perceived_soc() - 0.2).abs() < 1e-12,
            "precondition: the driver's belief must still be 0.2 — otherwise the estimate below \
             could come from the belief, not the observation (got {})",
            actor.perceived_soc()
        );

        let needed = actor
            .telemetry()
            .expect("driver telemetry")
            .get("needed_charge_hours")
            .expect("needed_charge_hours key");
        // Observed 0.6 is above the 0.2665 band: nothing to charge — 0.0 h.
        // A belief-sourced regression would report ≈0.66 h (0.2 → 0.2665).
        assert!(
            needed.abs() < 1e-9,
            "under anxiety, needed_charge_hours must report the observed gap to the band \
             (0.6 already above 0.2665 → 0 h), not the belief gap (0.2 → 0.2665 ≈ 0.66 h), got {needed}"
        );
    }

    /// The needed-charge-hours channel reports the observed battery gap in
    /// every phase — including Away, where the driver's belief is frozen by
    /// design after the trip-completion credit. During an away charging
    /// session the estimate must fall as the equipment's ground-truth SOC
    /// climbs; a value frozen at the driver's static belief would read as
    /// "no progress" through the whole workplace charge.
    #[test]
    fn needed_charge_hours_decreases_during_away_charging() {
        let mut actor = make_away_charge_actor(42);
        let mut out = Vec::new();

        // Depart at 08:00 and drive until the trip completes into Away.
        actor.decide(&env_at_minute(8 * 60), &mut out);
        for step in 1..=100 {
            out.clear();
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
            if matches!(actor.phase, DriverPhase::Away { .. }) {
                break;
            }
        }
        assert!(
            matches!(actor.phase, DriverPhase::Away { .. }),
            "precondition: the actor must be Away before the charging session"
        );

        // The deferred AwayPluggedIn + EvAwayCharge dispatch step.
        out.clear();
        actor.decide(&env_at_minute(8 * 60 + 101), &mut out);

        // A 60-step away-charging window at midday (12:00–12:59, well
        // before the 18:00 arrival): the equipment's ground-truth SOC
        // climbs 0.35 → 0.468 while it reports 6.6 kW of away charge power.
        let belief_at_window_start = actor.perceived_soc();
        let mut first_needed = None;
        let mut last_needed = 0.0;
        let mut any_charge: f64 = 0.0;
        for step in 0..60u16 {
            let mut env = env_at_minute(12 * 60 + step);
            let equipment_soc = 0.35 + f64::from(step) * 0.002;
            set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), equipment_soc);
            let mut away_telemetry = Telemetry::new();
            away_telemetry.insert(tk::AWAY_CHARGE_POWER_KW, 6.6);
            env.equipment_telemetry
                .insert("EV1".to_string(), away_telemetry);
            out.clear();
            actor.decide(&env, &mut out);

            let telemetry = actor.telemetry().expect("driver telemetry");
            any_charge = any_charge.max(telemetry.get("charge_kw").expect("charge_kw key"));
            let needed = telemetry
                .get("needed_charge_hours")
                .expect("needed_charge_hours key");
            first_needed.get_or_insert(needed);
            last_needed = needed;
        }

        let first_needed = first_needed.expect("window ran");
        assert!(
            any_charge > 0.0,
            "precondition: the equipment must be charging over the window (charge_kw max = {any_charge})"
        );
        assert!(
            (actor.perceived_soc() - belief_at_window_start).abs() < 1e-12,
            "precondition: the driver's belief must be static across the away window — otherwise \
             the fall could come from the belief, not the observation ({} → {})",
            belief_at_window_start,
            actor.perceived_soc()
        );
        assert!(
            last_needed < first_needed - 1.0,
            "needed_charge_hours must fall as the away session charges (observed SOC 0.35 → 0.468), \
             but it held {last_needed} against the window-start {first_needed} — frozen at the \
             driver's static belief while the equipment reports progress"
        );
    }

    #[test]
    fn nightly_strategy_idles_outside_window() {
        let mut actor = make_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.95,
            },
            PlugInPolicy::Always,
            42,
        );

        // Arrive at 18:00, check at 18:01 -- outside off-peak window (22:00-06:00)
        let out = drive_cycle_and_charge_step(&mut actor);

        assert!(
            !out.iter().any(commands_charging),
            "nightly must command no charging outside the off-peak window at 18:01, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
        assert!(
            out.iter().any(is_explicit_hold),
            "nightly outside its window must dispatch an explicit zero-power hold, not silence, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    #[test]
    fn low_soc_policy_skips_plug_in_when_above_threshold() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::LowSoc { threshold: 0.3 },
            42,
        );
        // SOC starts at 1.0, well above 0.3 threshold
        let mut out = Vec::new();

        // Depart
        actor.decide(&env_at_minute(8 * 60), &mut out);

        // Drive only a few steps (small drain, SOC stays high)
        out.clear();
        for step in 1..=5 {
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
        }

        // Skip to arrival -- SOC will be high since 30mi * 0.3kWh/mi = 9kWh out of 60kWh
        // That's 0.85 SOC, well above 0.3 threshold
        // Need to transition to Away phase first
        out.clear();
        for step in 6..=100 {
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
        }

        // Arrive
        out.clear();
        actor.decide(&env_at_minute(18 * 60), &mut out);

        let has_plug_in = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        assert!(
            !has_plug_in,
            "should NOT plug in when SOC ({}) is above threshold 0.3",
            actor.estimated_soc,
        );
    }

    #[test]
    fn daily_event_rolling_produces_correct_timing() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            123,
        );
        let mut out = Vec::new();

        // Trigger day roll
        let env = env_at_minute(0);
        actor.decide(&env, &mut out);

        // Event should be rolled since event_day_ratio=1.0
        assert!(
            actor.todays_event.is_some(),
            "event should be rolled with ratio=1.0"
        );

        let event = actor.todays_event.unwrap();
        // Arrival at 18:00 (1080 min), duration 10h (600 min) -> departure at 08:00 (480 min)
        assert_eq!(event.arrival_minute, 1080, "arrival should be 18:00");
        assert_eq!(event.departure_minute, 480, "departure should be 08:00");
        // 30mi × 0.3kWh/mi × temp_multiplier(10°C) ≈ 9.99 kWh
        let expected = 30.0 * 0.3 * temp_efficiency_multiplier(10.0);
        assert!(
            (event.drive_kwh - expected).abs() < 0.1,
            "drive_kwh should be ~{expected:.1}, got {}",
            event.drive_kwh,
        );
    }

    #[test]
    fn deterministic_same_seed_same_sequence() {
        let mut actor_a = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            999,
        );
        let mut actor_b = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            999,
        );

        let mut out_a = Vec::new();
        let mut out_b = Vec::new();

        for minute in [0, 8 * 60, 8 * 60 + 30, 18 * 60, 18 * 60 + 1] {
            let env = env_at_minute(minute);
            out_a.clear();
            out_b.clear();
            actor_a.decide(&env, &mut out_a);
            actor_b.decide(&env, &mut out_b);
            assert_eq!(
                out_a.len(),
                out_b.len(),
                "signal count mismatch at minute {minute}"
            );
            for (a, b) in out_a.iter().zip(out_b.iter()) {
                assert_eq!(
                    format!("{:?}", a.signal),
                    format!("{:?}", b.signal),
                    "signal content mismatch at minute {minute}"
                );
            }
        }

        // Internal state should match
        assert_eq!(
            actor_a.todays_event.map(|e| e.drive_kwh),
            actor_b.todays_event.map(|e| e.drive_kwh),
            "daily events should match with same seed"
        );
    }

    #[test]
    fn quick_then_wait_idles_above_threshold() {
        let mut actor = make_actor(
            ChargingStrategy::QuickThenWait { partial_soc: 0.5 },
            PlugInPolicy::Always,
            42,
        );

        let out = drive_cycle_and_charge_step(&mut actor);

        // After driving ~9kWh out of 60kWh, SOC ~ 0.83 > partial_soc 0.5.
        // SocGate overrides idle above threshold: no charging may be
        // commanded, and the idle must be an explicit hold — not silence,
        // which the equipment's BMS would fill with charge-to-full.
        assert!(
            !out.iter().any(commands_charging),
            "QuickThenWait should command no charging when SOC is above partial_soc=0.5, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
        assert!(
            out.iter().any(is_explicit_hold),
            "an idling QuickThenWait must dispatch an explicit zero-power hold, not silence, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    #[test]
    fn non_driving_day_dispatches_strategy_decision() {
        // A non-driving day is not a day off from charging control: the
        // composer runs on every plugged-in step, so the strategy's own
        // decision (here Immediate's SOCTarget) is dispatched exactly as on
        // a driving day. Going silent instead would leave any hold from the
        // previous evening latched through the day and let the equipment's
        // BMS default fill the gap.
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.event_day_ratio = 0.0; // never a driving day
        let mut out = Vec::new();

        let has_strategy_target = |out: &[DispatchRequest]| {
            out.iter().any(|r| {
                matches!(
                    r.signal,
                    ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 1e-9
                )
            })
        };

        actor.decide(&env_at_minute(8 * 60), &mut out);
        assert!(
            has_strategy_target(&out),
            "a non-driving day must dispatch the strategy's own decision (SOCTarget 0.9), got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );

        out.clear();
        actor.decide(&env_at_minute(18 * 60), &mut out);
        assert!(
            has_strategy_target(&out),
            "every plugged-in step of a non-driving day must carry the strategy decision, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    #[test]
    fn multi_step_driving_spreads_energy() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let mut out = Vec::new();

        // Depart
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();

        // Collect drive signals
        let mut total_drive_kwh = 0.0;
        let mut drive_count = 0;
        for step in 1..=120 {
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
            for req in &out {
                if let ControlSignal::EvDrive { kwh } = &req.signal {
                    total_drive_kwh += kwh;
                    drive_count += 1;
                }
            }
            out.clear();
        }

        assert!(
            drive_count > 1,
            "expected multi-step driving, got {drive_count} drive signals"
        );
        // 30mi × 0.3kWh/mi × temp_multiplier(10°C=1.11) ≈ 9.99 kWh
        let expected = 30.0 * 0.3 * temp_efficiency_multiplier(10.0);
        assert!(
            (total_drive_kwh - expected).abs() < 0.5,
            "total drive energy should be ~{expected:.1} kWh, got {total_drive_kwh}"
        );
    }

    #[test]
    fn range_anxiety_override_preempts_strategy_when_time_runs_short() {
        // A plugged-in Nightly driver at SOC 0.20 — below the 0.2958
        // anxiety band (the day's rolled 33.3 mi trip blended with the
        // buffer) but with a 12 kWh pack that still covers the 9.99 kWh
        // day's trip, so the departure proceeds (cancellation is pinned
        // separately for the uncoverable case).
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            0.20,
        );

        // 07:45, 15 minutes before the 08:00 departure and outside the
        // Nightly window: the driver is short of the band and out of time —
        // needed to the band ≈ 0.99 h × 1.2 ≈ 1.18 h ≫ 15 min — so the
        // override preempts the strategy's hold with the minimal
        // charge-to-band target.
        let out = plugged_in_step(&mut actor, 7 * 60 + 45);
        let has_band_charge = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. }
                    if (target_soc - anxiety_band_driving_day_at_10c()).abs() < 0.01
            )
        });
        assert!(
            has_band_charge,
            "15 minutes before departure, short of the band, the override must preempt the \
             (out-of-window) Nightly strategy with the minimal SOCTarget(band), got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );

        // 18:01 the same evening (the 08:00 departure has passed, so the
        // wrapped time to the next departure is ~14 h): the driver is still
        // below the band, but there is plenty of time — the urgency gate
        // stands the override down and the configured Nightly strategy
        // governs: an explicit hold outside the window, no anxious charge.
        let out = plugged_in_step(&mut actor, 18 * 60 + 1);
        let has_anxious_charge = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. }
                    if (target_soc - anxiety_band_driving_day_at_10c()).abs() < 0.01
            )
        });
        assert!(
            !has_anxious_charge,
            "with ~14 h until the next departure the override must stand down and let the \
             strategy govern, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
        assert!(
            out.iter().any(is_explicit_hold),
            "the Nightly strategy must hold outside its window while the override stands down, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    #[test]
    fn away_charge_fraction_emits_away_signals() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.away_charge_fraction = 0.3;
        actor.away_charge_power_kw = 11.5;
        let mut out = Vec::new();

        // Roll event, depart
        actor.decide(&env_at_minute(0), &mut out);
        out.clear();
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();

        // Drive through to completion, then step once more in Away phase
        // to trigger the deferred away-charge signals.
        for step in 1..=120 {
            out.clear();
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
            if matches!(actor.phase, DriverPhase::Away { .. }) && actor.needs_away_charge {
                // Transition to Away happened; next step emits deferred signals.
                break;
            }
        }

        // One more step in Away phase -- deferred signals are emitted here.
        out.clear();
        actor.decide(&env_at_minute(8 * 60 + 121), &mut out);

        let has_away_plug_in = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::AwayPluggedIn
                }
            )
        });
        let has_away_charge = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvAwayCharge { power_kw } if (power_kw - 11.5).abs() < 0.01
            )
        });
        assert!(
            has_away_plug_in,
            "should emit AwayPluggedIn when away_charge_fraction > 0"
        );
        assert!(
            has_away_charge,
            "should emit EvAwayCharge at configured power"
        );
    }

    #[test]
    fn v2h_strategy_charges_when_no_deficit() {
        let mut actor = make_actor(
            ChargingStrategy::V2H {
                discharge_threshold_soc: 0.8,
                min_soc: 0.2,
            },
            PlugInPolicy::Always,
            42,
        );

        let out = drive_cycle_and_charge_step(&mut actor);

        let has_soc_target = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget {
                    target_soc,
                    ..
                } if (target_soc - 0.9).abs() < 0.01
            )
        });
        assert!(
            has_soc_target,
            "V2H should emit SOCTarget(0.9), got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    /// At/below its `min_soc` floor the V2H discharge preference stands
    /// down (no power in its vote), but the floor must not pin the vehicle:
    /// the stack's own `SocTarget` still governs, so the composer dispatches
    /// the charge target — not the explicit zero-power hold an idle
    /// resolution would produce. A stack-level hold here is self-reinforcing
    /// (only charging raises SOC, and the hold blocks charging), so the
    /// vehicle would sit at its floor indefinitely.
    #[test]
    fn v2h_stack_charges_off_soc_floor() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2H {
                discharge_threshold_soc: 0.8,
                min_soc: 0.2,
            },
            0.15, // below the 0.2 floor: the discharge preference is idle
        );
        // 19:00 with the next departure ~13 h away: the range-anxiety
        // override has ample time and stands down, so the dispatch below is
        // the composer resolving the strategy's own stack.
        let out = plugged_in_step(&mut actor, 19 * 60);

        let has_strategy_target = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 0.01
            )
        });
        assert!(
            has_strategy_target,
            "at the soc floor the stack's SocTarget must still dispatch so the vehicle can \
             charge off the floor, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
        assert!(
            !out.iter().any(is_explicit_hold),
            "the floor must not resolve to a latching zero-power hold — that would pin the \
             vehicle at its floor indefinitely, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    // ======= TARIFF-011 composer integration tests =======

    #[test]
    fn ev_immediate_max_rate() {
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.4);
        let out = plugged_in_step(&mut actor, 19 * 60);

        let has_soc_target = out.iter().any(|r| {
            matches!(r.signal, ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 0.01)
        });
        assert!(
            has_soc_target,
            "Immediate should charge to 0.9, got: {out:?}"
        );
    }

    #[test]
    fn ev_not_plugged_in_no_dispatch() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        // Roll an event so todays_event is Some (arrival at 18:00 = 1080)
        let mut out = Vec::new();
        actor.decide(&env_at_minute(0), &mut out);
        out.clear();

        // Force Away phase with no pending away-charge signals, carrying
        // the rolled day's plan (arrival at 18:00 = 1080)
        let plan = actor.todays_event.expect("event rolled above");
        actor.phase = DriverPhase::Away { plan };
        actor.needs_away_charge = false;

        // Pick a minute that is NOT near arrival (1080). 15:00 = 900.
        actor.decide(&env_at_minute(15 * 60), &mut out);
        assert!(
            out.is_empty(),
            "Away phase should emit nothing when not at arrival minute, got: {out:?}"
        );
    }

    /// The `steps_left == 0` completion exit is the `Driving` arm's only
    /// early return in `decide`; it must populate telemetry like every
    /// other exit path, or the `phase` channel reads "Driving" for one
    /// step after the trip has actually completed into `Away` — a stale
    /// one-step reading on every trip completion (and after a mid-trip
    /// checkpoint restore that lands exactly on the completion boundary).
    #[test]
    fn trip_completion_step_publishes_away_phase_telemetry() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let plan = DayEvent {
            departure_minute: 480,
            arrival_minute: 1080,
            drive_kwh: 9.0,
        };
        actor.todays_event = Some(plan);

        // Mid-trip step: the phase channel reports Driving (1.0).
        actor.phase = DriverPhase::Driving {
            remaining_kwh: 9.0,
            total_steps: 4,
            steps_done: 2,
            plan,
        };
        let mut out = Vec::new();
        actor.decide(&env_at_minute(8 * 60), &mut out);
        assert_eq!(
            actor.telemetry().expect("driver telemetry").get("phase"),
            Some(1.0),
            "mid-trip step must report the Driving phase"
        );

        // Completion step: steps are already exhausted at arm entry (the
        // `steps_left == 0` exit). The transition to Away must be visible
        // in telemetry on this same step.
        actor.phase = DriverPhase::Driving {
            remaining_kwh: 0.0,
            total_steps: 4,
            steps_done: 4,
            plan,
        };
        out.clear();
        actor.decide(&env_at_minute(8 * 60 + 1), &mut out);
        assert!(
            matches!(actor.phase, DriverPhase::Away { .. }),
            "the completion step must transition to Away"
        );
        assert_eq!(
            actor.telemetry().expect("driver telemetry").get("phase"),
            Some(2.0),
            "the completion step must publish the Away phase, not leave the stale Driving reading"
        );
    }

    /// A trip whose driving steps cross midnight into a day that rolls
    /// non-driving must keep going: the trip is phase-carried state from
    /// the departure day, and the day roll decides only *future*
    /// departures. Pre-fix, `decide`'s non-driving-day branch returned
    /// before the phase arms ran, freezing the vehicle mid-route until the
    /// next driving day — no driving steps, no away charging, no arrival.
    #[test]
    fn midnight_crossing_trip_continues_into_non_driving_day() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.estimated_soc = 0.9;
        // Every rolled day is non-driving (deterministic — the assertion
        // path has no RNG dependence); the departure day's event is seeded
        // directly, exactly what the roll produces for a late departure.
        actor.event_day_ratio = 0.0;
        actor.away_charge_fraction = 0.5;
        actor.away_charge_power_kw = 6.6;
        let mut out = Vec::new();
        actor.decide(&env_at_day_minute_temp(1, 0, 10.0), &mut out);
        actor.todays_event = Some(DayEvent {
            departure_minute: 1425, // 23:45
            arrival_minute: 60,     // 01:00 the next day
            drive_kwh: 9.0,         // 30 mi × 0.3 kWh/mi → 60 one-minute steps
        });
        out.clear();

        // Depart at 23:45 — the trip's 60 steps span 23:46–00:45, crossing
        // the midnight roll into day 2.
        actor.decide(&env_at_day_minute_temp(1, 1425, 10.0), &mut out);
        assert!(
            matches!(actor.phase, DriverPhase::Driving { .. }),
            "precondition: the actor must depart at the event minute"
        );

        // Day 1 remainder of the trip.
        for minute in 1426..=1439 {
            out.clear();
            actor.decide(&env_at_day_minute_temp(1, minute, 10.0), &mut out);
        }

        // Midnight: day 2 rolls non-driving.
        out.clear();
        actor.decide(&env_at_day_minute_temp(2, 0, 10.0), &mut out);
        assert!(
            actor.todays_event.is_none(),
            "precondition: day 2 must have rolled non-driving"
        );
        assert!(
            out.iter()
                .any(|r| matches!(r.signal, ControlSignal::EvDrive { .. })),
            "a midnight-crossing trip must keep dispatching EvDrive on the \
             non-driving day — the trip is phase-carried, not tied to \
             todays_event; got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );

        // Drive to completion: the trip ends in Away with the away-charge
        // deferred signals pending.
        for minute in 1..=45 {
            out.clear();
            actor.decide(&env_at_day_minute_temp(2, minute, 10.0), &mut out);
        }
        assert!(
            matches!(actor.phase, DriverPhase::Away { .. }),
            "the trip must complete into Away on the non-driving day"
        );

        // The deferred away-charge signals fire on the next step — also on
        // a non-driving day.
        out.clear();
        actor.decide(&env_at_day_minute_temp(2, 46, 10.0), &mut out);
        assert!(
            out.iter()
                .any(|r| matches!(r.signal, ControlSignal::EvAwayCharge { .. })),
            "the deferred away-charge session must begin on the non-driving \
             day; got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );

        // Arrival home at the departed plan's own arrival minute (01:00),
        // not the next driving day's.
        out.clear();
        actor.decide(&env_at_day_minute_temp(2, 60, 10.0), &mut out);
        assert!(
            matches!(actor.phase, DriverPhase::HomePluggedIn),
            "the driver must arrive home at the departed trip's arrival minute"
        );
    }

    #[test]
    fn ev_nightly_off_peak_charges() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.95,
            },
            0.4,
        );
        let out = plugged_in_step(&mut actor, 23 * 60);

        let has_soc_target = out.iter().any(|r| {
            matches!(r.signal, ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.95).abs() < 0.01)
        });
        assert!(
            has_soc_target,
            "Nightly should charge during off-peak, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ev_nightly_peak_idles() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.95,
            },
            0.4,
        );
        let out = plugged_in_step(&mut actor, 19 * 60);

        assert!(
            !out.iter().any(commands_charging),
            "Nightly must command no charging during peak hours, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
        assert!(
            out.iter().any(is_explicit_hold),
            "Nightly during peak hours must dispatch an explicit zero-power hold, not silence, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ev_low_soc_below_threshold_charges() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::LowSoc {
                threshold: 0.5,
                target_soc: 0.8,
            },
            0.3,
        );
        let out = plugged_in_step(&mut actor, 19 * 60);

        let has_soc_target = out.iter().any(|r| {
            matches!(r.signal, ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.8).abs() < 0.01)
        });
        assert!(
            has_soc_target,
            "LowSoc should charge below threshold, got: {out:?}"
        );
    }

    #[test]
    fn ev_low_soc_above_threshold_idles() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::LowSoc {
                threshold: 0.5,
                target_soc: 0.8,
            },
            0.7,
        );
        let out = plugged_in_step(&mut actor, 19 * 60);

        assert!(
            !out.iter().any(commands_charging),
            "LowSoc must command no charging above its threshold, got: {out:?}"
        );
        assert!(
            out.iter().any(is_explicit_hold),
            "LowSoc holding above its threshold must dispatch an explicit zero-power hold, not silence, got: {out:?}"
        );
    }

    #[test]
    fn ev_tou_aware_charges_cheapest() {
        let mut prices: Vec<f64> = (0..24).map(|i| 0.10 + i as f64 * 0.01).collect();
        prices[2] = 0.01;
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![],
                charge_buffer_hours: 2.0,
            },
            0.4,
        );
        actor = actor.with_price_schedule(prices.into(), 24);
        actor.estimated_soc = 0.4;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut env = env_at_minute(2 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.01),
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let has_charge = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw > 0.0
            ) || matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if target_soc > 0.0
            )
        });
        assert!(has_charge, "TOU should charge at cheap price, got: {out:?}");
    }

    #[test]
    fn ev_tou_aware_buffer_fallback() {
        let prices: Vec<f64> = vec![0.10; 24];
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![],
                charge_buffer_hours: 2.0,
            },
            0.4,
        );
        actor = actor.with_price_schedule(prices.into(), 24);
        actor.estimated_soc = 0.4;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut env = env_at_minute(12 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.10),
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let has_signal = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { .. }
                    | ControlSignal::PowerSetpoint { .. }
                    | ControlSignal::EvSetReadyBy { .. }
            )
        });
        assert!(
            has_signal,
            "TOU with uniform prices should still charge, got: {out:?}"
        );
    }

    #[test]
    fn ev_solar_surplus_modulates_rate() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::SolarSurplus {
                min_charge_rate_kw: 1.0,
                departure_schedule: vec![],
            },
            0.4,
        );
        let mut env = env_at_minute(12 * 60);
        env.electrical = ElectricalSummary {
            pv_generation_kw: 5.0,
            base_load_kw: 1.5,
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let has_power = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if (active_power_kw - 3.5).abs() < 0.1
            )
        });
        assert!(
            has_power,
            "SolarSurplus should modulate to surplus (3.5kW), got: {out:?}"
        );
    }

    #[test]
    fn ev_solar_surplus_below_min_idles() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::SolarSurplus {
                min_charge_rate_kw: 1.0,
                departure_schedule: vec![],
            },
            0.4,
        );
        let mut env = env_at_minute(12 * 60);
        env.electrical = ElectricalSummary {
            pv_generation_kw: 2.0,
            base_load_kw: 1.5,
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let has_power = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw.abs() > 0.01
            )
        });
        assert!(
            !has_power,
            "SolarSurplus should idle below min rate, got: {out:?}"
        );
    }

    #[test]
    fn ev_pre_departure_defers_start() {
        use hares_types::{DayFilter, DepartureConstraint};
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::PreDeparture {
                target_soc: 0.9,
                departure_schedule: vec![DepartureConstraint {
                    day_filter: DayFilter::Any,
                    departure_minute: 420,
                    target_soc: 0.9,
                }],
            },
            0.8,
        );
        let out = plugged_in_step(&mut actor, 19 * 60);

        let has_signal = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { .. }
                    | ControlSignal::EvSetReadyBy { .. }
                    | ControlSignal::PowerSetpoint { .. }
            )
        });
        assert!(
            has_signal,
            "PreDeparture should plan charging, got: {out:?}"
        );
    }

    #[test]
    fn ev_pre_departure_charges_near_deadline() {
        use hares_types::{DayFilter, DepartureConstraint};
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::PreDeparture {
                target_soc: 0.9,
                departure_schedule: vec![DepartureConstraint {
                    day_filter: DayFilter::Any,
                    departure_minute: 420,
                    target_soc: 0.9,
                }],
            },
            0.2,
        );
        let out = plugged_in_step(&mut actor, 6 * 60);

        let has_urgent = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvSetReadyBy { .. } | ControlSignal::SOCTarget { .. }
            ) || matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw > 0.0
            )
        });
        assert!(
            has_urgent,
            "PreDeparture should urgently charge near deadline, got: {out:?}"
        );
    }

    #[test]
    fn ev_v2h_discharges_during_deficit() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2H {
                discharge_threshold_soc: 0.5,
                min_soc: 0.2,
            },
            0.7,
        );
        let mut env = env_at_minute(19 * 60);
        env.electrical = ElectricalSummary {
            pv_generation_kw: 1.0,
            base_load_kw: 4.0,
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let discharge_power = out.iter().find_map(|r| {
            if let ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } = &r.signal
            {
                if *active_power_kw < 0.0 {
                    Some(*active_power_kw)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(
            discharge_power.is_some(),
            "V2H should discharge during deficit, got: {out:?}"
        );
        let power = discharge_power.unwrap();
        assert!(
            (power + 3.0).abs() < 0.1,
            "V2H deficit = 4.0-1.0 = 3.0 kW, expected power ~ -3.0, got {power}"
        );
    }

    #[test]
    fn ev_v2h_power_setpoint_carries_min_soc_during_discharge() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2H {
                discharge_threshold_soc: 0.5,
                min_soc: 0.2,
            },
            0.7, // SOC above threshold (0.5), well above min_soc (0.2) — should discharge
        );
        let mut env = env_at_minute(19 * 60);
        env.electrical = ElectricalSummary {
            pv_generation_kw: 1.0,
            base_load_kw: 4.0,
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let min_soc_from_signal = out.iter().find_map(|r| {
            if let ControlSignal::PowerSetpoint {
                active_power_kw,
                min_soc,
                ..
            } = &r.signal
            {
                if *active_power_kw < 0.0 {
                    Some(*min_soc)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(
            min_soc_from_signal.is_some(),
            "V2H should emit PowerSetpoint with min_soc during discharge, got: {out:?}"
        );
        assert_eq!(
            min_soc_from_signal.unwrap(),
            Some(0.2),
            "PowerSetpoint should carry min_soc=0.2 from V2H strategy"
        );
    }

    #[test]
    fn ev_v2h_idles_with_low_soc() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2H {
                discharge_threshold_soc: 0.5,
                min_soc: 0.2,
            },
            0.15,
        );
        let mut env = env_at_minute(19 * 60);
        env.electrical = ElectricalSummary {
            pv_generation_kw: 1.0,
            base_load_kw: 4.0,
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let has_discharge = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw < -0.01
            )
        });
        assert!(
            !has_discharge,
            "V2H should not discharge below min_soc, got: {out:?}"
        );
    }

    #[test]
    fn ev_v2g_discharges_above_price() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2G {
                min_soc: 0.3,
                max_export_kw: 5.0,
                price_threshold: 0.20,
            },
            0.7,
        );
        let mut env = env_at_minute(19 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.30),
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let discharge_power = out.iter().find_map(|r| {
            if let ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } = &r.signal
            {
                if *active_power_kw < 0.0 {
                    Some(*active_power_kw)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(
            discharge_power.is_some(),
            "V2G should discharge above price threshold, got: {out:?}"
        );
        let power = discharge_power.unwrap();
        assert!(
            (power + 5.0).abs() < 0.1,
            "V2G max_export_kw=5.0, expected power ~ -5.0, got {power}"
        );
    }

    #[test]
    fn ev_v2g_idles_below_price() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2G {
                min_soc: 0.3,
                max_export_kw: 5.0,
                price_threshold: 0.20,
            },
            0.7,
        );
        let mut env = env_at_minute(19 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.10),
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let has_discharge = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw < -0.01
            )
        });
        assert!(
            !has_discharge,
            "V2G should not discharge below price threshold, got: {out:?}"
        );
    }

    fn make_away_charge_actor(seed: u64) -> EvDriverActor {
        EvDriverActor::new(
            "AwayDriver",
            "EV1",
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            ScheduleSource::Constant(30.0),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(600.0),
            None,
            1.0,
            0.3,
            60.0,
            7.2,
            30.0,
            20.0,
            0.5, // 50% away charge
            6.6,
            seed_from_u64(seed),
        )
    }

    #[test]
    fn away_charge_signals_deferred_to_next_step() {
        let mut actor = make_away_charge_actor(42);
        let mut out = Vec::new();

        // Depart at 08:00
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();

        // Drive until trip completes
        let mut final_drive_signals = Vec::new();
        for step in 1..=100 {
            out.clear();
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
            if matches!(actor.phase, DriverPhase::Away { .. }) {
                final_drive_signals = out.clone();
                break;
            }
        }

        // Final driving step: EvDrive present, AwayPluggedIn absent
        let has_drive = final_drive_signals
            .iter()
            .any(|r| matches!(r.signal, ControlSignal::EvDrive { .. }));
        let has_away_plugin = final_drive_signals.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::AwayPluggedIn
                }
            )
        });
        assert!(has_drive, "final driving step should emit EvDrive");
        assert!(
            !has_away_plugin,
            "final driving step must NOT emit AwayPluggedIn (deferred)"
        );

        // Next step in Away: deferred signals emitted
        out.clear();
        actor.decide(&env_at_minute(8 * 60 + 101), &mut out);
        let has_away_now = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::AwayPluggedIn
                }
            )
        });
        let has_away_charge = out
            .iter()
            .any(|r| matches!(r.signal, ControlSignal::EvAwayCharge { .. }));
        assert!(
            has_away_now,
            "first Away step should emit deferred AwayPluggedIn"
        );
        assert!(
            has_away_charge,
            "first Away step should emit deferred EvAwayCharge"
        );

        // Subsequent Away step: no re-emission
        out.clear();
        actor.decide(&env_at_minute(8 * 60 + 102), &mut out);
        let has_away_again = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::AwayPluggedIn
                }
            )
        });
        assert!(
            !has_away_again,
            "subsequent Away steps should not re-emit AwayPluggedIn"
        );
    }

    #[test]
    fn expected_daily_miles_from_schedule_mean() {
        let actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        assert_eq!(actor.expected_daily_miles, 30.0);
    }

    // H2: Range anxiety fires on a non-driving day when SOC is critically low.
    //
    // On a non-driving day todays_event is None, so decide() routes through
    // evaluate_charging, which tries the range-anxiety override before the
    // composer. We test needs_range_anxiety_override() directly (accessible
    // from within the same module's test block) to confirm the predicate is
    // true when SOC is below the anxiety threshold regardless of whether a
    // trip is scheduled.
    #[test]
    fn range_anxiety_triggers_on_non_driving_day_with_low_soc() {
        // Actor: 30 mi/day expected, 20 mi buffer, 0.3 kWh/mi, 60 kWh battery.
        // anxiety_kwh = (30 + 20) * 0.3 * temp_mult(10°C)
        // anxiety_soc = anxiety_kwh / 60.0
        // At temp 10°C, temp_mult ≈ 1.11, anxiety_kwh ≈ 16.65, anxiety_soc ≈ 0.278
        // SOC 0.15 < 0.278 → should trigger.
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );

        actor.estimated_soc = 0.15;
        actor.todays_event = None;
        actor.phase = DriverPhase::HomePluggedIn;

        let env = env_at_minute(0);
        assert!(
            actor.needs_range_anxiety_override(&env),
            "needs_range_anxiety_override should be true when SOC=0.15 is below anxiety threshold \
             (expected_daily_miles={}, range_anxiety_miles={}, fuel_economy={}, capacity={})",
            actor.expected_daily_miles,
            actor.range_anxiety_miles,
            actor.fuel_economy_kwh_per_mi,
            actor.capacity_kwh,
        );
    }

    // ======= Range anxiety on non-driving days =======

    #[test]
    fn non_driving_day_range_anxiety_emits_soc_target() {
        // Actor: 30 mi/day expected, 20 mi buffer, 0.3 kWh/mi, 60 kWh battery.
        // anxiety_soc ≈ 0.278 at 10°C. SOC 0.15 < 0.278 → should trigger.
        // Nightly window (22:00–06:00): the 08:00 step is outside it, so the
        // composer resolves no target and the override governs — under the
        // precedence rule an in-window strategy target ≥ band would stand
        // the override down by design.
        let mut actor = make_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            PlugInPolicy::Always,
            42,
        );
        actor.event_day_ratio = 0.0;
        actor.estimated_soc = 0.15;
        actor.todays_event = None;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut out = Vec::new();
        actor.decide(&env_at_minute(8 * 60), &mut out);

        let has_soc_target = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. }
                    if (target_soc - anxiety_band_non_driving_day_at_10c()).abs() < 0.01
            )
        });
        assert!(
            has_soc_target,
            "non-driving day with low SOC: decide() must emit the minimal SOCTarget(band) via range anxiety, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    /// The precedence rule's stand-down side: when the driver is short
    /// (perceived SOC below the anxiety band) but the strategy's own
    /// resolved target for the step already covers the band, the override
    /// must NOT pre-empt it — dispatching the band target instead of a
    /// higher configured target charges less than the driver asked for,
    /// for zero safety gain. The composer's plan is the step's dispatch
    /// (path exclusivity: never both).
    ///
    /// Not reachable through real assembly (working observation reconciles
    /// the belief far above the band before anxiety can trigger), so the
    /// vehicle is this actor-level harness with a hand-set belief — the
    /// defect under test is the actor's precedence logic, not assembly
    /// identity.
    #[test]
    fn range_anxiety_stands_down_when_strategy_target_covers_band() {
        // Immediate is always in-window: the composer resolves target 0.9,
        // far above the ~0.25 non-driving-day band at 10 °C.
        let mut actor =
            make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.15);
        actor.event_day_ratio = 0.0;
        actor.todays_event = None;
        let mut env = env_at_minute(12 * 60);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.15);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        // The composer's higher target is the step's charging dispatch...
        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 1e-9
            )),
            "with the strategy target (0.9) covering the band, the strategy's own plan must be dispatched, got {out:?}"
        );
        // ...and the override's minimal band target must not also appear —
        // a step's charging dispatch comes from exactly one path.
        let band = anxiety_band_non_driving_day_at_10c();
        assert!(
            !out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - band).abs() < 1e-9
            )),
            "the range-anxiety override must stand down when the strategy's own target covers the band (band = {band}), got {out:?}"
        );
    }

    /// The stand-down rule keys on the composer's resolved *target* SOC.
    /// But a rate-bearing strategy (TouAware) can resolve a target far
    /// above the band while simultaneously resolving a *discharge* rate at
    /// peak price: the pack is then not being charged toward that target —
    /// it is being driven away from the band. "The band is already covered"
    /// is false on such a step, so the range-anxiety override — whose job is
    /// to top a short pack up to the band — must still fire. Standing down
    /// silently disables the backstop the ticket names ("at minimum the
    /// range-anxiety backstop charges an empty pack") on a pack below the
    /// band.
    #[test]
    fn range_anxiety_must_fire_when_strategy_target_is_not_backed_by_charging() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![],
                charge_buffer_hours: 2.0,
            },
            0.15,
        );
        let prices: Vec<f64> = (0..24).map(|i| 0.10 + i as f64 * 0.01).collect();
        actor = actor.with_price_schedule(prices.clone().into(), 24);
        actor.event_day_ratio = 0.0;
        actor.todays_event = None;
        actor.estimated_soc = 0.15;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut env = env_at_minute(23 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(prices[23]),
            ..Default::default()
        };
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        let band = anxiety_band_non_driving_day_at_10c();
        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - band).abs() < 1e-9
            )),
            "with the pack below the anxiety band and the strategy discharging at peak \
             price, the range-anxiety override must fire and dispatch the band target; \
             the strategy's nominal 0.9 target does not cover the band while its resolved \
             rate is negative. got {out:?}"
        );
        assert!(
            !out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw < 0.0
            )),
            "the same step must not discharge a pack already below the anxiety band, \
             got {out:?}"
        );
    }

    /// The rate condition's positive arm: a strategy actively charging
    /// toward its own band-covering target (TouAware at a cheap price —
    /// target 0.9 from the stack, rate +max from the price vote) must keep
    /// the override stood down, and the composer's positive-rate plan must
    /// dispatch on that step (path exclusivity: the strategy governs). A
    /// transposed comparison (`< 0.0`) would fire the override on exactly
    /// this step and preempt an in-progress charge with the lower band
    /// target.
    #[test]
    fn range_anxiety_stands_down_when_strategy_actively_charges_toward_its_target() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![],
                charge_buffer_hours: 2.0,
            },
            0.15,
        );
        let prices: Vec<f64> = (0..24).map(|i| 0.10 + i as f64 * 0.01).collect();
        actor = actor.with_price_schedule(prices.clone().into(), 24);
        actor.event_day_ratio = 0.0;
        actor.todays_event = None;
        actor.estimated_soc = 0.15;
        actor.phase = DriverPhase::HomePluggedIn;

        // Minute 0 carries the schedule's minimum price — strictly at or
        // below the charge threshold, so the price vote resolves a
        // positive charge rate.
        let mut env = env_at_minute(0);
        env.price_signal = PriceSignal {
            electricity_price: Some(prices[0]),
            ..Default::default()
        };
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        let band = anxiety_band_non_driving_day_at_10c();
        assert!(
            !out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - band).abs() < 1e-9
            )),
            "the strategy is charging toward its own 0.9 target at a cheap price — \
             the override must not preempt it with the band target, got {out:?}"
        );
        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw > 0.0
            )),
            "the composer's positive-rate charge plan must dispatch on the step \
             the override stands down (the strategy governs), got {out:?}"
        );
    }

    /// The rate condition's zero boundary (`> 0.0`, not `>= 0.0`). No real
    /// strategy stack resolves a band-covering target paired with an
    /// exactly-zero rate (SolarTracking, the only zero-rate emitter, is
    /// stacked only with DepartureDeadline, whose target requires a
    /// today-event — which either arms the urgency gate or flips the
    /// departure vote to a positive-rate override), so the boundary is
    /// pinned with fixed-vote preferences — the same synthetic-preference
    /// pattern the composer's own tests use — constructing exactly the
    /// resolution under test: target 0.9 (covers the band) + rate 0.0 (a
    /// hold). A zero-rate hold does not charge toward the target, so the
    /// band is NOT covered and the override must fire; a `>= 0.0`
    /// regression silently stands the backstop down on holds.
    struct FixedVotePref(PreferenceVote);
    impl ChargingPreference for FixedVotePref {
        fn score(&mut self, _: &DecisionContext) -> PreferenceVote {
            self.0.clone()
        }
        fn name(&self) -> &'static str {
            "fixed_vote_test"
        }
    }

    #[test]
    fn range_anxiety_fires_when_strategy_target_is_paired_with_a_zero_rate_hold() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.composer = ChargingComposer::new(
            vec![
                Box::new(FixedVotePref(PreferenceVote {
                    target_soc: Some(0.9),
                    power_kw: None,
                    departure_hour: None,
                    min_soc: None,
                    max_soc: None,
                    score: 1.0,
                    label: "test:target",
                })),
                Box::new(FixedVotePref(PreferenceVote {
                    target_soc: None,
                    power_kw: Some(0.0),
                    departure_hour: None,
                    min_soc: None,
                    max_soc: None,
                    score: 1.5,
                    label: "test:zero_hold",
                })),
            ],
            actor.target_name(),
        );
        actor.event_day_ratio = 0.0;
        actor.todays_event = None;
        actor.estimated_soc = 0.15;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut out = Vec::new();
        actor.decide(&env_at_minute(12 * 60), &mut out);

        let band = anxiety_band_non_driving_day_at_10c();
        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - band).abs() < 1e-9
            )),
            "the resolved plan holds at exactly zero rate — the pack is not being \
             driven toward the 0.9 target, so the band is not covered and the \
             override must fire (the rate boundary is > 0, not >= 0), got {out:?}"
        );
    }

    /// A strategy target at or above the anxiety band whose resolved vote
    /// caps the pack BELOW the band (`max_soc` — the composer's
    /// most-restrictive upper-bound fold) is not "covered": the equipment
    /// stops charging at the cap while the target reads high — the same
    /// "pack held away from the band" geometry the rate-backing check
    /// exists for, in the dimension the check does not read. The override
    /// must fire; standing down strands the driver on a pack capped below
    /// the next trip's need.
    #[test]
    fn range_anxiety_fires_when_strategy_target_caps_below_the_band_via_max_soc() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.composer = ChargingComposer::new(
            vec![Box::new(FixedVotePref(PreferenceVote {
                target_soc: Some(0.9),
                power_kw: None,
                departure_hour: None,
                min_soc: None,
                max_soc: Some(0.1),
                score: 1.0,
                label: "test:sub_band_cap",
            }))],
            actor.target_name(),
        );
        actor.event_day_ratio = 0.0;
        actor.todays_event = None;
        actor.estimated_soc = 0.15;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut out = Vec::new();
        actor.decide(&env_at_minute(12 * 60), &mut out);

        let band = anxiety_band_non_driving_day_at_10c();
        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, max_soc, .. }
                    if (target_soc - band).abs() < 1e-9
                        && max_soc.is_none_or(|cap| cap >= band)
            )),
            "a target of 0.9 capped by max_soc = 0.1 charges only to 0.1 — below \
             the band ({band}) — so the band is not covered and the override \
             must fire; standing down strands the driver, got {out:?}"
        );
    }

    /// The cap condition's stand-down boundary: a resolved cap that sits
    /// between the band and the target (a price-tier ceiling of 0.5 over a
    /// 0.9 target on a pack at 0.15 with a ~0.28 band) still covers the
    /// band — the pack charges to 0.5, comfortably above the next trip's
    /// need — so the strategy's own plan governs and the override must
    /// stand down. An over-broad cap check (`cap >= target`, or firing on
    /// any cap at all) would preempt the strategy's charge every step and
    /// lock the pack at the minimal band target instead.
    #[test]
    fn range_anxiety_stands_down_when_the_resolved_cap_still_covers_the_band() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.composer = ChargingComposer::new(
            vec![Box::new(FixedVotePref(PreferenceVote {
                target_soc: Some(0.9),
                power_kw: None,
                departure_hour: None,
                min_soc: None,
                max_soc: Some(0.5),
                score: 1.0,
                label: "test:band_covering_cap",
            }))],
            actor.target_name(),
        );
        actor.event_day_ratio = 0.0;
        actor.todays_event = None;
        actor.estimated_soc = 0.15;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut out = Vec::new();
        actor.decide(&env_at_minute(12 * 60), &mut out);

        let band = anxiety_band_non_driving_day_at_10c();
        assert!(
            !out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - band).abs() < 1e-9
            )),
            "a cap of 0.5 over a 0.9 target still charges past the band ({band}) — \
             the strategy's plan covers the band and the override must stand \
             down, got {out:?}"
        );
        assert!(
            out.iter().any(|r| matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, max_soc, .. }
                    if (target_soc - 0.9).abs() < 1e-9
                        && max_soc.is_some_and(|cap| (cap - 0.5).abs() < 1e-9)
            )),
            "on stand-down the composer's own capped plan must dispatch — the \
             ceiling (0.9) and its cap (0.5) both reaching the equipment — \
             got {out:?}"
        );
    }

    #[test]
    fn non_driving_day_no_anxiety_when_soc_high() {
        // SOC well above the anxiety threshold: the override stands down and
        // the strategy's own decision is what dispatches — not silence.
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.event_day_ratio = 0.0;
        actor.estimated_soc = 0.8;
        actor.todays_event = None;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut out = Vec::new();
        actor.decide(&env_at_minute(8 * 60), &mut out);

        let has_anxiety_minimal_charge = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. }
                    if (target_soc - anxiety_band_non_driving_day_at_10c()).abs() < 1e-9
            )
        });
        assert!(
            !has_anxiety_minimal_charge,
            "non-driving day with SOC above the anxiety threshold must not fire the override, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
        assert!(
            out.iter().any(commands_charging),
            "with the override standing down, the strategy's own decision must dispatch, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    #[test]
    fn non_driving_day_anxiety_charges_for_next_driving_day() {
        // Regression: PlugInPolicy::LowSoc with a non-driving day at low SOC
        // must charge the battery so the driver is not stranded on the
        // following driving day. Nightly window (22:00–06:00): the Day-1
        // 08:00 step is outside it, so the composer resolves no target and
        // the override governs — under the precedence rule an in-window
        // strategy target ≥ band would stand the override down by design.
        let mut actor = make_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            PlugInPolicy::LowSoc { threshold: 0.5 },
            42,
        );

        // --- Day 1: non-driving day, critically low SOC ---
        actor.event_day_ratio = 0.0;
        actor.estimated_soc = 0.15;
        actor.phase = DriverPhase::HomePluggedIn;
        actor.todays_event = None;

        let mut out = Vec::new();
        actor.decide(&env_at_minute(8 * 60), &mut out);

        assert!(
            out.iter().any(|r| {
                matches!(
                    r.signal,
                    ControlSignal::SOCTarget { target_soc, .. }
                        if (target_soc - anxiety_band_non_driving_day_at_10c()).abs() < 0.01
                )
            }),
            "Day 1 (non-driving): range anxiety must charge to the band (trip + reserve)"
        );

        // Simulate overnight charging: battery is now full.
        actor.estimated_soc = 1.0;

        // --- Day 2: driving day ---
        actor.current_day_ordinal = -1;
        actor.event_day_ratio = 1.0;

        // Depart, drive, arrive, then step inside the Nightly window
        // (22:30) where the strategy's own target governs.
        let mut out = Vec::new();
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();
        for step in 1..=600 {
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
            out.clear();
            if matches!(actor.phase, DriverPhase::HomePluggedIn) {
                break;
            }
        }
        assert!(
            matches!(actor.phase, DriverPhase::HomePluggedIn),
            "precondition: the actor must be home and plugged in before the evening step"
        );
        let day2_out = plugged_in_step(&mut actor, 22 * 60 + 30);

        // Range anxiety should NOT have fired — started Day 2 at full SOC.
        let has_anxiety = day2_out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. }
                    if (target_soc - anxiety_band_non_driving_day_at_10c()).abs() < 0.01
            )
        });
        assert!(
            !has_anxiety,
            "Day 2 range anxiety should not fire — battery was fully charged on Day 1"
        );

        // Normal charging should proceed (Immediate targets 0.9).
        let has_normal = day2_out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 0.01
            )
        });
        assert!(
            has_normal,
            "Day 2 should charge normally to 0.9 (battery was full on departure)"
        );
    }

    // ======= Compound preference interaction tests =======

    #[test]
    fn ev_tou_with_departure_deadline_overrides_price() {
        use hares_types::{DayFilter, DepartureConstraint};

        // TouAware with a departure deadline in 1 hour, SOC at 0.3, target 0.9.
        // Even though the price is expensive, departure urgency should override.
        let prices: Vec<f64> = (0..24).map(|i| i as f64 * 0.02).collect();
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![DepartureConstraint {
                    day_filter: DayFilter::Any,
                    departure_minute: 7 * 60, // 07:00
                    target_soc: 0.9,
                }],
                charge_buffer_hours: 2.0,
            },
            0.3,
        );
        actor = actor.with_price_schedule(prices.into(), 24);
        actor.estimated_soc = 0.3;
        actor.phase = DriverPhase::HomePluggedIn;

        // At 06:00, only 1 hour until departure. Default env temp=10°C → effective_eff=0.811
        // Need 0.6*60/(7.2*0.811) ≈ 6.19h. 1h << 6.19h * 1.2 = urgent override fires.
        let mut env = env_at_minute(6 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.40), // expensive
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let has_positive_power = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw > 0.0
            ) || matches!(r.signal, ControlSignal::EvSetReadyBy { .. })
                || matches!(
                    r.signal,
                    ControlSignal::SOCTarget { target_soc, .. } if target_soc >= 0.9
                )
        });
        assert!(
            has_positive_power,
            "departure deadline should override price optimizer and force charging, got: {out:?}"
        );
    }

    #[test]
    fn ev_solar_surplus_defers_then_deadline_forces() {
        use hares_types::{DayFilter, DepartureConstraint};

        let mut actor = make_plugged_in_actor(
            ChargingStrategy::SolarSurplus {
                min_charge_rate_kw: 1.4,
                departure_schedule: vec![DepartureConstraint {
                    day_filter: DayFilter::Any,
                    departure_minute: 7 * 60,
                    target_soc: 1.0,
                }],
            },
            0.3,
        );

        // Step 1: no PV surplus, 8 hours until departure (23:00 -> 07:00 = 8h).
        // Default env temp=10°C → effective_eff=0.811
        // SOC 0.3, need 0.7*60/(7.2*0.811) ≈ 7.19h, 8h > 7.19 * 1.2 = not urgent.
        // Solar has nothing -> should idle.
        let mut env1 = env_at_minute(23 * 60);
        env1.electrical = ElectricalSummary {
            pv_generation_kw: 0.0,
            base_load_kw: 1.0,
            ..Default::default()
        };
        let out1 = plugged_in_step_with_env(&mut actor, &env1);
        // No PowerSetpoint should fire -- solar has no surplus.
        // EvSetReadyBy may still be emitted (departure planning), which is fine --
        // it tells the BMS when to be ready, not to charge now.
        let has_active_charging1 = out1.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw > 0.01
            )
        });
        assert!(
            !has_active_charging1,
            "solar surplus with no PV should not emit active PowerSetpoint, got: {out1:?}"
        );

        // Step 2: same conditions but only 30 min until departure (06:30).
        // Need ≈7.19h but only 0.5h -> departure override fires.
        let env2 = env_at_minute(6 * 60 + 30);
        let out2 = plugged_in_step_with_env(&mut actor, &env2);
        let has_charging2 = out2.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw > 0.0
            ) || matches!(r.signal, ControlSignal::EvSetReadyBy { .. })
        });
        assert!(
            has_charging2,
            "departure deadline should force max-rate charging, got: {out2:?}"
        );
    }

    #[test]
    fn ev_v2h_then_charges_when_cheap() {
        // V2H: evening with high load, low PV, SOC above threshold -> should discharge.
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2H {
                discharge_threshold_soc: 0.5,
                min_soc: 0.2,
            },
            0.7,
        );

        // Evening: high load deficit
        let mut env_evening = env_at_minute(19 * 60);
        env_evening.electrical = ElectricalSummary {
            pv_generation_kw: 0.5,
            base_load_kw: 4.0,
            ..Default::default()
        };
        let out_evening = plugged_in_step_with_env(&mut actor, &env_evening);
        let has_discharge = out_evening.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw < 0.0
            )
        });
        assert!(
            has_discharge,
            "V2H should discharge during evening deficit, got: {out_evening:?}"
        );

        // Morning: no load deficit, SOC now at 0.4 (below discharge threshold 0.5).
        // V2H idles on discharge; SocTarget(0.9) should emit SOCTarget.
        actor.estimated_soc = 0.4;
        let mut env_morning = env_at_minute(6 * 60);
        env_morning.electrical = ElectricalSummary {
            pv_generation_kw: 3.0,
            base_load_kw: 1.0,
            ..Default::default()
        };
        let out_morning = plugged_in_step_with_env(&mut actor, &env_morning);
        let has_charge = out_morning.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 0.01
            )
        });
        assert!(
            has_charge,
            "V2H should charge via SocTarget when SOC is low and no deficit, got: {out_morning:?}"
        );
    }

    #[test]
    fn ev_v2g_respects_min_soc_floor() {
        // V2G: high price but SOC exactly at min_soc. Floor constraint should prevent discharge.
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2G {
                min_soc: 0.3,
                max_export_kw: 5.0,
                price_threshold: 0.20,
            },
            0.3, // exactly at min_soc
        );

        let mut env = env_at_minute(19 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.50), // well above threshold
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let has_discharge = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw < -0.01
            )
        });
        assert!(
            !has_discharge,
            "V2G should not discharge at min_soc floor, got: {out:?}"
        );
    }

    #[test]
    fn ev_tou_prefers_cheap_over_expensive() {
        // Two steps through the full actor: one at cheap price, one at expensive.
        let prices: Vec<f64> = (0..24).map(|i| i as f64 * 0.02).collect();
        // 25th percentile ~ prices[6] = 0.12, 75th percentile ~ prices[18] = 0.36
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![],
                charge_buffer_hours: 2.0,
            },
            0.5,
        );
        actor = actor.with_price_schedule(prices.into(), 24);
        actor.estimated_soc = 0.5;
        actor.phase = DriverPhase::HomePluggedIn;

        // Cheap price step
        let mut env_cheap = env_at_minute(2 * 60);
        env_cheap.price_signal = PriceSignal {
            electricity_price: Some(0.02),
            ..Default::default()
        };
        let out_cheap = plugged_in_step_with_env(&mut actor, &env_cheap);
        let has_charge = out_cheap.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw > 0.0
            ) || matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if target_soc > 0.0
            )
        });
        assert!(
            has_charge,
            "TOU should charge at cheap price, got: {out_cheap:?}"
        );

        // Expensive price step: PriceOptimizer scores a discharge (negative
        // PowerSetpoint) because price 0.44 > discharge_threshold (~0.34).
        // The behavioral difference: cheap price → positive charge PowerSetpoint,
        // expensive price → negative discharge PowerSetpoint. The resolved
        // vote also carries the strategy's standing SOCTarget ceiling —
        // dispatched alongside the rate, not a charge instruction — so the
        // ceiling must not count as charging.
        let mut env_expensive = env_at_minute(20 * 60);
        env_expensive.price_signal = PriceSignal {
            electricity_price: Some(0.44),
            ..Default::default()
        };
        let out_expensive = plugged_in_step_with_env(&mut actor, &env_expensive);
        let has_positive_charge = out_expensive.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw > 0.01
            )
        });
        assert!(
            !has_positive_charge,
            "TOU must not command charging at expensive price (should discharge instead), got: {out_expensive:?}"
        );
        let has_discharge = out_expensive.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw < -0.01
            )
        });
        assert!(
            has_discharge,
            "TOU should discharge at expensive price, got: {out_expensive:?}"
        );
    }

    // ======= Realistic scenario tests =======

    #[test]
    fn ev_nightly_full_cycle() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.95,
            },
            0.5,
        );

        // Evening 18:00-21:59: outside off-peak window, must hold
        for hour in 18..22 {
            let out = plugged_in_step(&mut actor, hour * 60);
            assert!(
                !out.iter().any(commands_charging),
                "nightly must command no charging at hour {hour} (before off-peak), got: {:?}",
                out.iter().map(|r| &r.signal).collect::<Vec<_>>()
            );
            assert!(
                out.iter().any(is_explicit_hold),
                "nightly before off-peak must dispatch an explicit hold at hour {hour}, got: {:?}",
                out.iter().map(|r| &r.signal).collect::<Vec<_>>()
            );
        }

        // Overnight 22:00-05:59: off-peak, should charge
        for hour in [22, 23, 0, 1, 2, 3, 4, 5] {
            let out = plugged_in_step(&mut actor, hour * 60);
            let has_charging = out.iter().any(|r| {
                matches!(
                    r.signal,
                    ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.95).abs() < 0.01
                )
            });
            assert!(
                has_charging,
                "nightly should charge at hour {hour} (off-peak), got: {:?}",
                out.iter().map(|r| &r.signal).collect::<Vec<_>>()
            );
        }

        // Morning 06:00-07:59: outside off-peak again, must hold
        for hour in 6..8 {
            let out = plugged_in_step(&mut actor, hour * 60);
            assert!(
                !out.iter().any(commands_charging),
                "nightly must command no charging at hour {hour} (after off-peak), got: {:?}",
                out.iter().map(|r| &r.signal).collect::<Vec<_>>()
            );
            assert!(
                out.iter().any(is_explicit_hold),
                "nightly after off-peak must dispatch an explicit hold at hour {hour}, got: {:?}",
                out.iter().map(|r| &r.signal).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn ev_immediate_charges_every_step() {
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.3);

        for step in 0..5 {
            let minute = 19 * 60 + step;
            let out = plugged_in_step(&mut actor, minute);
            let has_soc_target = out.iter().any(|r| {
                matches!(
                    r.signal,
                    ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 0.01
                )
            });
            assert!(
                has_soc_target,
                "Immediate should charge at every step (step {step}), got: {out:?}"
            );
        }
    }

    #[test]
    fn ev_solar_surplus_tracks_pv_curve() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::SolarSurplus {
                min_charge_rate_kw: 1.4,
                departure_schedule: vec![],
            },
            0.3,
        );

        // 0 kW PV -> idle
        let mut env0 = env_at_minute(12 * 60);
        env0.electrical = ElectricalSummary {
            pv_generation_kw: 0.0,
            base_load_kw: 1.5,
            ..Default::default()
        };
        let out0 = plugged_in_step_with_env(&mut actor, &env0);
        assert!(
            out0.is_empty() || !out0.iter().any(|r| matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw.abs() > 0.01
            )),
            "should idle with 0 kW PV, got: {out0:?}"
        );

        // 1.0 kW PV, 1.5 kW load -> surplus -0.5 kW (below min 1.4), idle
        let mut env1 = env_at_minute(12 * 60 + 1);
        env1.electrical = ElectricalSummary {
            pv_generation_kw: 1.0,
            base_load_kw: 1.5,
            ..Default::default()
        };
        let out1 = plugged_in_step_with_env(&mut actor, &env1);
        assert!(
            out1.is_empty() || !out1.iter().any(|r| matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw.abs() > 0.01
            )),
            "should idle with insufficient surplus (1.0-1.5=-0.5), got: {out1:?}"
        );

        // 3.0 kW PV, 1.5 kW load -> surplus 1.5 kW (above min 1.4), charge at 1.5 kW
        let mut env2 = env_at_minute(12 * 60 + 2);
        env2.electrical = ElectricalSummary {
            pv_generation_kw: 3.0,
            base_load_kw: 1.5,
            ..Default::default()
        };
        let out2 = plugged_in_step_with_env(&mut actor, &env2);
        let power2 = out2.iter().find_map(|r| {
            if let ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } = &r.signal
            {
                Some(*active_power_kw)
            } else {
                None
            }
        });
        assert!(
            power2.is_some() && (power2.unwrap() - 1.5).abs() < 0.1,
            "should charge at ~1.5 kW surplus, got: {out2:?}"
        );

        // 5.0 kW PV, 1.5 kW load -> surplus 3.5 kW, charge at 3.5 kW
        let mut env3 = env_at_minute(12 * 60 + 3);
        env3.electrical = ElectricalSummary {
            pv_generation_kw: 5.0,
            base_load_kw: 1.5,
            ..Default::default()
        };
        let out3 = plugged_in_step_with_env(&mut actor, &env3);
        let power3 = out3.iter().find_map(|r| {
            if let ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } = &r.signal
            {
                Some(*active_power_kw)
            } else {
                None
            }
        });
        assert!(
            power3.is_some() && (power3.unwrap() - 3.5).abs() < 0.1,
            "should charge at ~3.5 kW surplus, got: {out3:?}"
        );
    }

    // ======= Edge case tests =======

    #[test]
    fn ev_missing_price_signal_tou_falls_back() {
        let prices: Vec<f64> = (0..24).map(|i| i as f64 * 0.02).collect();
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![],
                charge_buffer_hours: 2.0,
            },
            0.4,
        );
        actor = actor.with_price_schedule(prices.into(), 24);
        actor.estimated_soc = 0.4;
        actor.phase = DriverPhase::HomePluggedIn;

        // No price signal (None) -- should not panic
        let mut env = env_at_minute(12 * 60);
        env.price_signal = PriceSignal {
            electricity_price: None,
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);
        // PriceOptimizer uses unwrap_or(0.0) for None, so price=0.0 <= charge_threshold.
        // With SOC=0.4 and target=0.9, SocTarget fallback should still emit.
        let has_signal = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { .. } | ControlSignal::PowerSetpoint { .. }
            )
        });
        assert!(
            has_signal,
            "TOU with missing price should still emit via fallback, got {} signals: {out:?}",
            out.len()
        );
    }

    #[test]
    fn ev_missing_electrical_summary_solar_idles() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::SolarSurplus {
                min_charge_rate_kw: 1.0,
                departure_schedule: vec![],
            },
            0.4,
        );

        // Default ElectricalSummary has all zeros
        let env = env_at_minute(12 * 60);
        let out = plugged_in_step_with_env(&mut actor, &env);

        let has_power = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw.abs() > 0.01
            )
        });
        assert!(
            !has_power,
            "SolarSurplus should idle with zero PV/load, got: {out:?}"
        );
    }

    #[test]
    fn ev_soc_at_target_still_emits_soc_target() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            0.9, // already at target
        );

        let out = plugged_in_step(&mut actor, 19 * 60);

        // SocTarget score = (0.9 - 0.9).max(0.0) = 0.0, so score is 0.
        // emit_vote with target_soc=0.9 but score=0 still emits SOCTarget.
        // The key is that the equipment BMS handles the fact that SOC == target.
        // At the actor level, a zero-gap SOCTarget is still valid to emit.
        // However, per the resolve logic a score of 0 still wins if it's the
        // only vote, and target_soc is set. So it emits SOCTarget(0.9).
        // This verifies the actor doesn't panic or misbehave at target.
        let has_soc_target = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 0.01
            )
        });
        // At target SOC, score is 0 but SOCTarget is still emitted (equipment handles no-op).
        assert!(
            has_soc_target,
            "Immediate at target SOC should still emit SOCTarget (equipment handles no-op), got: {out:?}"
        );
    }

    // ======= Test gap audit findings =======

    // Finding 5: build_preferences never directly asserted
    #[test]
    fn build_preferences_tou_aware_installs_three_preferences() {
        let prefs = build_preferences(
            &ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![],
                charge_buffer_hours: 2.0,
            },
            7.2,
            0.9,
            None,
            24,
        );
        assert_eq!(
            prefs.len(),
            3,
            "TouAware should install 3 preferences (PriceOptimizer + DepartureDeadline + SocTarget)"
        );
    }

    #[test]
    fn build_preferences_immediate_installs_one() {
        let prefs = build_preferences(
            &ChargingStrategy::Immediate { target_soc: 0.9 },
            7.2,
            0.9,
            None,
            24,
        );
        assert_eq!(
            prefs.len(),
            1,
            "Immediate should install 1 preference (SocTarget)"
        );
    }

    #[test]
    fn build_preferences_solar_surplus_installs_two() {
        let prefs = build_preferences(
            &ChargingStrategy::SolarSurplus {
                min_charge_rate_kw: 1.0,
                departure_schedule: vec![],
            },
            7.2,
            0.9,
            None,
            24,
        );
        assert_eq!(
            prefs.len(),
            2,
            "SolarSurplus should install 2 preferences (SolarTracking + DepartureDeadline)"
        );
    }

    // Finding 6: evaluate_charging path not proven
    #[test]
    fn evaluate_charging_updates_last_action() {
        let mut actor = make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.5);

        let out = plugged_in_step(&mut actor, 19 * 60);
        assert!(
            !out.is_empty(),
            "plugged-in actor with SOC<target should emit signal"
        );
        let action1 = actor.last_action().to_owned();
        assert!(
            action1.starts_with("resolved:") || action1.starts_with("override:"),
            "last_action should start with resolved: or override:, got: {action1}"
        );

        // Step again at different SOC
        actor.estimated_soc = 0.95;
        let _out2 = plugged_in_step(&mut actor, 19 * 60 + 1);
        let action2 = actor.last_action().to_owned();
        assert!(
            !action2.is_empty(),
            "last_action should be set after second step"
        );
    }

    // Finding 7: Departure override label not pinned
    #[test]
    fn ev_tou_with_departure_deadline_override_label_pinned() {
        use hares_types::{DayFilter, DepartureConstraint};

        let prices: Vec<f64> = (0..24).map(|i| i as f64 * 0.02).collect();
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![DepartureConstraint {
                    day_filter: DayFilter::Any,
                    departure_minute: 7 * 60,
                    target_soc: 0.9,
                }],
                charge_buffer_hours: 2.0,
            },
            0.3,
        );
        actor = actor.with_price_schedule(prices.into(), 24);
        actor.estimated_soc = 0.3;
        actor.phase = DriverPhase::HomePluggedIn;

        let mut env = env_at_minute(6 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.40),
            ..Default::default()
        };
        let _out = plugged_in_step_with_env(&mut actor, &env);

        assert!(
            actor.last_action().contains("override") || actor.last_action().contains("departure"),
            "expected departure override path, got: {}",
            actor.last_action()
        );
    }

    // Finding 8: SocTarget score-driven priority
    #[test]
    fn tou_neutral_price_soc_target_drives_charging() {
        // Neutral price (between charge and discharge thresholds), no departure
        // pressure. Only SocTarget provides a non-zero score. SOC level drives
        // whether a charging signal is emitted.
        // Spread prices 0..0.23 → charge_threshold(25th)≈0.06, discharge_threshold(75th)≈0.17
        let prices: Vec<f64> = (0..24).map(|i| i as f64 * 0.01).collect();

        // Low SOC: gap = 0.9 - 0.4 = 0.5 → SocTarget scores 0.5 → emits SOCTarget(0.9)
        let mut actor_low = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![],
                charge_buffer_hours: 0.0,
            },
            0.4,
        );
        actor_low = actor_low.with_price_schedule(prices.clone().into(), 24);
        actor_low.estimated_soc = 0.4;
        actor_low.phase = DriverPhase::HomePluggedIn;

        // High SOC: gap = 0.9 - 0.95 = 0.0 → SocTarget scores 0 → idle-ish
        let mut actor_high = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![],
                charge_buffer_hours: 0.0,
            },
            0.95,
        );
        actor_high = actor_high.with_price_schedule(prices.into(), 24);
        actor_high.estimated_soc = 0.95;
        actor_high.phase = DriverPhase::HomePluggedIn;

        // Price 0.10: between 0.06 (charge threshold) and 0.17 (discharge threshold) → neutral
        let mut env = env_at_minute(12 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.10),
            ..Default::default()
        };

        let out_low = plugged_in_step_with_env(&mut actor_low, &env);
        let _out_high = plugged_in_step_with_env(&mut actor_high, &env);

        // Low SOC: SocTarget(0.5 score) wins → SOCTarget(0.9)
        let has_soc_target = out_low.iter().any(|r| {
            matches!(r.signal, ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 0.01)
        });
        assert!(
            has_soc_target,
            "low SOC (0.4) with neutral price should emit SOCTarget(0.9), got: {out_low:?}"
        );

        // High SOC: every vote scores 0. resolve()'s tie-break lets a later
        // tied vote's target win over the earlier idle vote's absence of
        // one, so the resolved vote still carries SocTarget's target 0.9 —
        // a no-op at the equipment (SOC 0.95 is above target), but the
        // strategy's ceiling is asserted rather than silently dropped.
        // The key: low SOC produces a meaningful charging signal (score > 0).
        let action_low = actor_low.last_action();
        assert!(
            action_low.contains("soc_target"),
            "at neutral price, SocTarget should drive the resolved action, got: {action_low}"
        );
    }

    // Finding 1 (integration): TouAware buffer_hours forces charge
    #[test]
    fn ev_tou_aware_buffer_hours_forces_charge() {
        use hares_types::{DayFilter, DepartureConstraint};

        // Expensive price, departure 5h away. Without buffer, price optimizer
        // would push toward discharge/neutral. With buffer=6, departure:buffer fires.
        let prices: Vec<f64> = (0..24).map(|i| i as f64 * 0.03).collect();

        let mut actor = make_plugged_in_actor(
            ChargingStrategy::TouAware {
                target_soc: 0.9,
                departure_schedule: vec![DepartureConstraint {
                    day_filter: DayFilter::Any,
                    departure_minute: 7 * 60, // 07:00
                    target_soc: 0.9,
                }],
                charge_buffer_hours: 6.0,
            },
            0.5,
        );
        actor = actor.with_price_schedule(prices.into(), 24);
        actor.estimated_soc = 0.5;
        actor.phase = DriverPhase::HomePluggedIn;

        // At 02:00, 5 hours until departure. buffer_hours=6 > 5h → buffer fires.
        // SOC=0.5 < target=0.9 → Override(departure:buffer)
        let mut env = env_at_minute(2 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.50), // expensive
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let has_positive_charge = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw > 0.0
            ) || matches!(r.signal, ControlSignal::EvSetReadyBy { .. })
                || matches!(
                    r.signal,
                    ControlSignal::SOCTarget { target_soc, .. } if target_soc >= 0.9
                )
        });
        assert!(
            has_positive_charge,
            "buffer_hours=6 should force charging despite expensive price, got: {out:?}"
        );
        assert!(
            actor.last_action().contains("departure") && actor.last_action().contains("buffer"),
            "last_action should indicate departure buffer override, got: {}",
            actor.last_action()
        );
    }

    #[test]
    fn resolve_equipment_id_missing_name_sets_equipment_id_to_none() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let id_by_name: HashMap<String, EquipmentId> = HashMap::new();
        actor.resolve_equipment_id(&id_by_name);
        assert!(
            actor.equipment_id.is_none(),
            "equipment_id must be None when target name ('EV1') is not in the registry"
        );
    }

    #[test]
    fn resolve_equipment_id_found_name_sets_equipment_id() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let mut id_by_name = HashMap::new();
        id_by_name.insert("EV1".to_string(), EquipmentId(7));
        actor.resolve_equipment_id(&id_by_name);
        assert_eq!(
            actor.equipment_id,
            Some(EquipmentId(7)),
            "equipment_id must match the registry entry"
        );
    }

    /// Step the actor through all 1440 minutes of a day, recording the signals
    /// emitted at each phase transition and the SOC evolution.
    ///
    /// Actor config: 30 mi/day, 0.3 kWh/mi, 60 kWh battery, depart 08:00 (480),
    /// arrive 18:00 (1080), average speed 30 mph.  event_day_ratio = 1.0.
    ///
    /// Physics assertions:
    /// - Departure minute emits `EvPlugIn { Disconnected }`.
    /// - At least one `EvDrive` signal is emitted between departure and arrival.
    /// - Total `EvDrive` kWh matches expected drive energy within 5 %.
    /// - Actor's `estimated_soc` decreases monotonically across driving steps
    ///   and ends lower than the pre-departure value.
    /// - Arrival minute emits `EvPlugIn { HomePluggedIn }`.
    /// - Post-arrival HomePluggedIn step emits a charging signal (`SOCTarget`).
    /// - `estimated_soc` after arrival is below the pre-departure SOC (energy
    ///   was consumed; the actor has not yet been told charging completed).
    #[test]
    fn full_24h_lifecycle_soc_and_connection_transitions() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );

        // ---- bookkeeping across the full day --------------------------------
        let mut departure_signals: Vec<ControlSignal> = Vec::new();
        let mut arrival_signals: Vec<ControlSignal> = Vec::new();
        let mut post_arrival_signals: Vec<ControlSignal> = Vec::new();
        let mut total_drive_kwh = 0.0_f64;
        let mut drive_signal_count = 0_usize;

        // SOC samples: (minute, estimated_soc)
        let mut soc_at_pre_departure = 1.0_f64;
        let mut soc_samples_driving: Vec<f64> = Vec::new();
        let mut soc_at_post_arrival = f64::NAN;

        let mut out = Vec::new();
        let mut saw_departure = false;
        let mut saw_arrival = false;
        let mut saw_post_arrival = false;

        // ---- simulate 1440 steps (minutes 0..1439) --------------------------
        for minute in 0_u16..1440 {
            let env = env_at_minute(minute);
            out.clear();
            actor.decide(&env, &mut out);

            if minute == 479 {
                // Snapshot SOC immediately before departure step runs.
                soc_at_pre_departure = actor.estimated_soc;
            }

            if minute == 480 && !saw_departure {
                saw_departure = true;
                departure_signals = out.iter().map(|r| r.signal.clone()).collect();
            }

            // Collect driving energy and SOC samples between departure and arrival.
            if minute > 480 && minute < 1080 {
                for req in &out {
                    if let ControlSignal::EvDrive { kwh } = req.signal {
                        total_drive_kwh += kwh;
                        drive_signal_count += 1;
                        soc_samples_driving.push(actor.estimated_soc);
                    }
                }
            }

            if minute == 1080 && !saw_arrival {
                saw_arrival = true;
                arrival_signals = out.iter().map(|r| r.signal.clone()).collect();
            }

            if minute == 1081 && !saw_post_arrival {
                saw_post_arrival = true;
                post_arrival_signals = out.iter().map(|r| r.signal.clone()).collect();
                soc_at_post_arrival = actor.estimated_soc;
            }
        }

        // ---- 1. Departure emits Disconnected --------------------------------
        let has_disconnect = departure_signals.iter().any(|s| {
            matches!(
                s,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::Disconnected
                }
            )
        });
        assert!(
            has_disconnect,
            "minute 480 (departure) must emit EvPlugIn{{Disconnected}}, got: {departure_signals:?}"
        );

        // ---- 2. EvDrive signals were emitted during the trip ----------------
        assert!(
            drive_signal_count > 1,
            "expected multi-step EvDrive signals between departure and arrival, got {drive_signal_count}"
        );

        // ---- 3. Total drive energy matches physics within 5% ----------------
        // 30 mi × 0.3 kWh/mi × temp_multiplier(10 °C outdoor)
        let expected_kwh = 30.0 * 0.3 * temp_efficiency_multiplier(10.0);
        let pct_err = ((total_drive_kwh - expected_kwh) / expected_kwh).abs();
        assert!(
            pct_err < 0.05,
            "total EvDrive kWh ({total_drive_kwh:.3}) should be within 5% of expected ({expected_kwh:.3})"
        );

        // ---- 4. estimated_soc decreased monotonically across driving steps --
        // Each driving step spreads energy evenly; the actor deducts from
        // estimated_soc after each step, so the sequence must be non-increasing.
        assert!(
            !soc_samples_driving.is_empty(),
            "expected SOC samples from driving steps"
        );
        for i in 1..soc_samples_driving.len() {
            assert!(
                soc_samples_driving[i] <= soc_samples_driving[i - 1] + 1e-9,
                "estimated_soc must be non-increasing across driving steps: \
                 sample[{i}]={} > sample[{}]={}",
                soc_samples_driving[i],
                i - 1,
                soc_samples_driving[i - 1],
            );
        }

        // ---- 5. Post-arrival SOC is below pre-departure SOC -----------------
        // Drive consumed energy; the actor hasn't been told charging completed.
        assert!(
            soc_at_post_arrival < soc_at_pre_departure - 1e-6,
            "estimated_soc after arrival ({soc_at_post_arrival:.4}) must be below \
             pre-departure SOC ({soc_at_pre_departure:.4})"
        );

        // ---- 6. Arrival emits HomePluggedIn ---------------------------------
        let has_home_plugin = arrival_signals.iter().any(|s| {
            matches!(
                s,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        assert!(
            has_home_plugin,
            "minute 1080 (arrival) must emit EvPlugIn{{HomePluggedIn}}, got: {arrival_signals:?}"
        );

        // ---- 7. Post-arrival step emits a charging signal -------------------
        let has_charging_signal = post_arrival_signals.iter().any(|s| {
            matches!(
                s,
                ControlSignal::SOCTarget { target_soc, .. } if (*target_soc - 0.9).abs() < 0.01
            )
        });
        assert!(
            has_charging_signal,
            "minute 1081 (post-arrival HomePluggedIn) must emit SOCTarget(0.9), \
             got: {post_arrival_signals:?}"
        );
    }

    // ── Arrival-time sampling tests ───────────────────────────────────

    #[test]
    fn direct_arrival_sampling_emits_signals_at_sampled_arrival_time() {
        // Actor with arrival_time set to Constant(900) (15:00). Verify that
        // arrival signals fire at 15:00, not at departure+duration=18:00.
        let mut actor = EvDriverActor::new(
            "TestDriver",
            "EV1",
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            ScheduleSource::Constant(30.0),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(600.0),
            Some(ScheduleSource::Constant(900.0)),
            1.0,
            0.3,
            60.0,
            7.2,
            30.0,
            20.0,
            0.0,
            0.0,
            seed_from_u64(42),
        );

        let mut out = Vec::new();

        // Depart at 08:00.
        actor.decide(&env_at_minute(480), &mut out);
        assert!(
            out.iter().any(|d| matches!(
                d.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::Disconnected
                }
            )),
            "departure at 08:00 should emit Disconnected"
        );
        out.clear();

        // Drive through to 14:58.
        for step in 481..=898 {
            actor.decide(&env_at_minute(step), &mut out);
            out.clear();
        }

        // At 14:59: should not have arrived yet.
        actor.decide(&env_at_minute(899), &mut out);
        let early = out.iter().any(|d| {
            matches!(
                d.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        assert!(
            !early,
            "should NOT emit HomePluggedIn before sampled arrival at 15:00"
        );
        out.clear();

        // At 15:00: arrival fires.
        actor.decide(&env_at_minute(900), &mut out);
        let has_arrival = out.iter().any(|d| {
            matches!(
                d.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        assert!(
            has_arrival,
            "direct arrival at 15:00 should emit HomePluggedIn, got: {out:?}"
        );
    }

    #[test]
    fn no_arrival_params_falls_back_to_departure_plus_duration() {
        // Actor with arrival_time=None: arrival = departure + trip_duration.
        // departure=480, duration=420 → arrival=900 (15:00).
        let mut actor = EvDriverActor::new(
            "TestDriver",
            "EV1",
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            ScheduleSource::Constant(30.0),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(420.0),
            None,
            1.0,
            0.3,
            60.0,
            7.2,
            30.0,
            20.0,
            0.0,
            0.0,
            seed_from_u64(42),
        );

        let mut out = Vec::new();

        actor.decide(&env_at_minute(480), &mut out);
        assert!(out.iter().any(|d| matches!(
            d.signal,
            ControlSignal::EvPlugIn {
                state: EvConnectionState::Disconnected
            }
        )));
        out.clear();

        for step in 481..=898 {
            actor.decide(&env_at_minute(step), &mut out);
            out.clear();
        }

        // At 14:59: not yet arrived.
        actor.decide(&env_at_minute(899), &mut out);
        let early = out.iter().any(|d| {
            matches!(
                d.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        assert!(
            !early,
            "should NOT emit HomePluggedIn before 15:00 (departure+duration)"
        );
        out.clear();

        // At 15:00: arrival via fallback path.
        actor.decide(&env_at_minute(900), &mut out);
        let has_arrival = out.iter().any(|d| {
            matches!(
                d.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        assert!(
            has_arrival,
            "fallback arrival at 15:00 should emit HomePluggedIn, got: {out:?}"
        );
    }

    /// Verify that direct sampling produces a different arrival time than
    /// the derived path would for the same departure. Two actors with
    /// same departure but different arrival config diverge.
    #[test]
    fn direct_sampling_arrival_differs_from_derived() {
        // Actor A: arrival_time=Constant(1020) — arrives 17:00.
        // Actor B: arrival_time=None, duration=Constant(420) — arrives 15:00.
        let mut actor_direct = EvDriverActor::new(
            "DriverDirect",
            "EV1",
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            ScheduleSource::Constant(30.0),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(420.0),
            Some(ScheduleSource::Constant(1020.0)),
            1.0,
            0.3,
            60.0,
            7.2,
            30.0,
            20.0,
            0.0,
            0.0,
            seed_from_u64(42),
        );

        let mut actor_derived = EvDriverActor::new(
            "DriverDerived",
            "EV1",
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            ScheduleSource::Constant(30.0),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(420.0),
            None,
            1.0,
            0.3,
            60.0,
            7.2,
            30.0,
            20.0,
            0.0,
            0.0,
            seed_from_u64(43),
        );

        let mut out = Vec::new();

        // Depart both.
        actor_direct.decide(&env_at_minute(480), &mut out);
        out.clear();
        actor_derived.decide(&env_at_minute(480), &mut out);
        out.clear();

        // Drive both through to 14:59.
        for step in 481..=899 {
            actor_direct.decide(&env_at_minute(step), &mut out);
            out.clear();
            actor_derived.decide(&env_at_minute(step), &mut out);
            out.clear();
        }

        // At 15:00: derived actor arrives, direct actor does NOT.
        actor_direct.decide(&env_at_minute(900), &mut out);
        let direct_1500 = out.iter().any(|d| {
            matches!(
                d.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        out.clear();

        actor_derived.decide(&env_at_minute(900), &mut out);
        let derived_1500 = out.iter().any(|d| {
            matches!(
                d.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        out.clear();

        assert!(
            !direct_1500,
            "direct actor should NOT arrive at 15:00 (arrival set to 17:00)"
        );
        assert!(
            derived_1500,
            "derived actor should arrive at 15:00 (departure 08:00 + 7h)"
        );

        // Drive direct actor to 17:00 and verify arrival.
        for step in 901..=1019 {
            actor_direct.decide(&env_at_minute(step), &mut out);
            out.clear();
        }

        actor_direct.decide(&env_at_minute(1020), &mut out);
        let direct_1700 = out.iter().any(|d| {
            matches!(
                d.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        assert!(
            direct_1700,
            "direct actor should arrive at 17:00, got: {out:?}"
        );
    }

    #[test]
    fn save_state_load_state_round_trip_estimated_soc_preserved() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.estimated_soc = 0.73;
        actor.current_day_ordinal = 12345;
        actor.phase = DriverPhase::HomePluggedIn;

        let blob = actor.save_state().expect("save_state should succeed");
        assert!(
            !blob.is_empty(),
            "stateful actor must produce non-empty blob"
        );

        let mut restored = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        restored
            .load_state(&blob)
            .expect("load_state should succeed");

        assert!((restored.estimated_soc - 0.73).abs() < 1e-12);
        assert_eq!(restored.current_day_ordinal, 12345);
        assert_eq!(restored.phase, DriverPhase::HomePluggedIn);
    }

    /// The snapshot carries every mutable decision field; a field added to
    /// `EvDriverSnapshot` but forgotten in `load_state` compiles fine and
    /// silently resets to the constructor default across a checkpoint — the
    /// sibling round-trip test only pins three fields. This pins the full
    /// snapshot, including the cumulative `drive_cancelled` counter (a
    /// cancelled trip must stay counted across a checkpoint/restart, not
    /// re-depart as if never cancelled) and the away-charge flag.
    #[test]
    fn save_state_load_state_round_trip_full_snapshot_preserved() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.estimated_soc = 0.61;
        actor.current_day_ordinal = 77;
        actor.phase = DriverPhase::Away {
            plan: DayEvent {
                departure_minute: 480,
                arrival_minute: 1080,
                drive_kwh: 9.5,
            },
        };
        actor.needs_away_charge = true;
        actor.drive_cancelled = 2;
        actor.todays_event = Some(DayEvent {
            departure_minute: 480,
            arrival_minute: 1080,
            drive_kwh: 9.5,
        });

        let blob = actor.save_state().expect("save_state should succeed");

        let mut restored = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        restored
            .load_state(&blob)
            .expect("load_state should succeed");

        assert!((restored.estimated_soc - 0.61).abs() < 1e-12);
        assert_eq!(restored.current_day_ordinal, 77);
        assert_eq!(
            restored.phase,
            DriverPhase::Away {
                plan: DayEvent {
                    departure_minute: 480,
                    arrival_minute: 1080,
                    drive_kwh: 9.5,
                },
            }
        );
        assert!(restored.needs_away_charge);
        assert_eq!(
            restored.drive_cancelled, 2,
            "a cancelled trip must stay counted across a checkpoint/restart"
        );
        let event = restored
            .todays_event
            .expect("todays_event must survive the round trip");
        assert_eq!(event.departure_minute, 480);
        assert_eq!(event.arrival_minute, 1080);
        assert!((event.drive_kwh - 9.5).abs() < 1e-12);
    }

    /// The snapshot round-trip must also preserve the *mid-trip* phase
    /// variant: `DriverPhase::Driving` now carries the departed day's plan
    /// (`remaining_kwh`, step counters, and the `DayEvent`), and a field
    /// dropped or reset in `load_state` would silently zero the trip's
    /// remaining energy across every checkpoint taken mid-route — the
    /// vehicle would arrive home with phantom range or stall the trip.
    /// The sibling full-snapshot test populates only `Away { plan }`.
    #[test]
    fn snapshot_round_trip_preserves_mid_trip_driving_plan() {
        let plan = DayEvent {
            departure_minute: 1380, // 23:00 — a midnight-crossing trip
            arrival_minute: 60,
            drive_kwh: 12.0,
        };
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.phase = DriverPhase::Driving {
            remaining_kwh: 7.5,
            total_steps: 80,
            steps_done: 20,
            plan,
        };

        let blob = actor.save_state().expect("save_state should succeed");
        let mut restored = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        restored
            .load_state(&blob)
            .expect("load_state should succeed");

        match restored.phase {
            DriverPhase::Driving {
                remaining_kwh,
                total_steps,
                steps_done,
                plan: restored_plan,
            } => {
                assert!(
                    (remaining_kwh - 7.5).abs() < 1e-12,
                    "the trip's remaining energy must survive the round trip, got {remaining_kwh}"
                );
                assert_eq!(total_steps, 80, "total steps must survive the round trip");
                assert_eq!(steps_done, 20, "steps done must survive the round trip");
                assert_eq!(
                    restored_plan, plan,
                    "the departed day's plan must survive the round trip — the arrival \
                     minute and drive budget travel with the phase"
                );
            }
            other => panic!("a mid-trip snapshot must restore as Driving, got {other:?}"),
        }
    }

    #[test]
    fn post_restore_estimated_soc_not_reset_to_default() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.estimated_soc = 0.45;

        let blob = actor.save_state().expect("save_state should succeed");

        let mut restored = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        restored
            .load_state(&blob)
            .expect("load_state should succeed");

        assert!(
            (restored.estimated_soc - 0.45).abs() < 1e-12,
            "estimated_soc should be 0.45 after restore, not the default 1.0"
        );
    }

    #[test]
    fn actor_with_no_state_returns_empty_blob() {
        struct NoStateActor;
        impl Actor for NoStateActor {
            fn name(&self) -> &str {
                "none"
            }
            fn decide(&mut self, _: &EnvironmentState, _: &mut Vec<DispatchRequest>) {}
        }
        let actor = NoStateActor;
        let blob = actor
            .save_state()
            .expect("save_state default should succeed");
        assert!(blob.is_empty());
        let mut actor = NoStateActor;
        actor
            .load_state(&[])
            .expect("load_state default should succeed");
    }

    // ======= SOC divergence model tests =======

    #[test]
    fn should_plug_in_uses_perceived_soc_not_equipment_core_telemetry() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::LowSoc { threshold: 0.4 },
            42,
        );
        // estimated_soc is 1.0 by default (well above 0.4 threshold)
        actor.phase = DriverPhase::HomePluggedIn;
        // Insert equipment core with SOC=0.1 (below threshold)
        let mut env = env_at_minute(18 * 60);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.1);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        let has_plug_in = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        assert!(
            !has_plug_in,
            "should NOT plug in when estimated_soc is above threshold even if actual SOC is low"
        );
    }

    #[test]
    fn perceived_soc_diverges_from_actual_during_driving() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        // Set equipment_core with SOC=0.5, estimated_soc=0.8 — simulating
        // a case where the driver's naive energy accounting overestimates SOC.
        actor.estimated_soc = 0.8;
        let mut env = env_at_minute(12 * 60);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.5);
        let perceived = actor.perceived_soc();
        let actual = actor.actual_soc(&env);
        assert!((perceived - 0.8).abs() < 1e-12);
        assert_eq!(actual, Some(0.5));
        assert!(
            (perceived - 0.5).abs() > 0.05,
            "perceived SOC (0.8) should diverge from actual equipment SOC (0.5)"
        );
    }

    #[test]
    fn needs_range_anxiety_override_uses_perceived_soc() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        // expected_daily_miles=30, range_anxiety_miles=20, fuel=0.3, cap=60
        // anxiety_soc ≈ (30+20)*0.3*1.11/60 ≈ 0.278
        // perceived estimated_soc=0.10 < anxiety threshold → should trigger
        actor.estimated_soc = 0.10;
        let mut env = env_at_minute(0);
        // Insert equipment_core with high SOC=0.9 — driver doesn't know this
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.9);
        assert!(
            actor.needs_range_anxiety_override(&env),
            "range anxiety should use perceived_soc (0.10), not actual equipment SOC (0.9)"
        );
    }

    #[test]
    fn evaluate_charging_uses_perceived_soc_for_decision_context() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        // estimated_soc=0.4, equipment_core says SOC=0.95
        actor.estimated_soc = 0.4;
        actor.phase = DriverPhase::HomePluggedIn;
        let mut env = env_at_minute(18 * 60 + 1);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.95);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        let has_soc_target = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if target_soc >= 0.9
            ) || matches!(r.signal, ControlSignal::EvSetReadyBy { .. })
                || matches!(
                    r.signal,
                    ControlSignal::PowerSetpoint { active_power_kw, .. } if active_power_kw > 0.0
                )
        });
        assert!(
            has_soc_target,
            "should charge when perceived_soc (0.4) is below target, even if actual SOC (0.95) is at target"
        );
    }

    #[test]
    fn multi_day_estimated_soc_reconciled_at_each_arrival() {
        // Simulate multiple complete drive cycles. At each arrival the
        // driver reconciles estimated_soc to actual equipment SOC, so
        // divergence does not accumulate across days.
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let mut env = env_at_minute(0);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 1.0);
        for _cycle in 0..3 {
            let mut out = Vec::new();
            for minute in 0_u16..1440 {
                env.current_time = env_at_minute(minute).current_time;
                out.clear();
                actor.decide(&env, &mut out);
            }
        }
        let perceived = actor.perceived_soc();
        let actual = actor.actual_soc(&env);
        assert!(
            (perceived - actual.unwrap()).abs() < 1e-6,
            "after 3 cycles with reconciliation, estimated_soc should equal actual equipment SOC"
        );
        assert_eq!(
            actual,
            Some(1.0),
            "equipment_core SOC should remain unchanged at 1.0"
        );
    }

    // ======= SOC reconciliation tests (T-0429) =======

    #[test]
    fn home_plugged_in_transition_reconciles_estimated_soc_to_actual() {
        use chrono::TimeZone;

        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let mut env = env_at_minute(0);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.73);
        actor.estimated_soc = 0.45;

        let mut out = Vec::new();

        // Step through minutes 0 to 1081 (departure at 480, arrival at 1080).
        for minute in 0_u16..=1081 {
            let h = (minute / 60) as u8;
            let m = (minute % 60) as u8;
            let tz = chrono::FixedOffset::east_opt(0).unwrap();
            env.current_time = tz
                .with_ymd_and_hms(2026, 1, 1, h as u32, m as u32, 0)
                .single()
                .unwrap();

            out.clear();
            actor.decide(&env, &mut out);

            if minute == 1080 {
                assert_eq!(
                    actor.phase,
                    DriverPhase::HomePluggedIn,
                    "phase should be HomePluggedIn after arrival"
                );
                assert!(
                    (actor.estimated_soc - 0.73).abs() < 1e-6,
                    "estimated_soc should be reconciled to actual equipment SOC 0.73 after arrival, got {}",
                    actor.estimated_soc
                );
            }
        }
    }

    #[test]
    fn low_soc_policy_reconciliation_fires_on_arrival_without_plug_in() {
        use chrono::TimeZone;

        // When PlugInPolicy::LowSoc with threshold=0.3 prevents plug-in
        // (perceived SOC 0.80 > 0.3), the phase still transitions to
        // HomePluggedIn and reconciliation still fires.
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::LowSoc { threshold: 0.3 },
            42,
        );
        let mut env = env_at_minute(0);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 0.82);
        actor.estimated_soc = 0.80;

        let mut out = Vec::new();

        for minute in 0_u16..=1080 {
            let h = (minute / 60) as u8;
            let m = (minute % 60) as u8;
            let tz = chrono::FixedOffset::east_opt(0).unwrap();
            env.current_time = tz
                .with_ymd_and_hms(2026, 1, 1, h as u32, m as u32, 0)
                .single()
                .unwrap();

            out.clear();
            actor.decide(&env, &mut out);

            if minute == 1080 {
                let has_plugin = out.iter().any(|r| {
                    matches!(
                        r.signal,
                        ControlSignal::EvPlugIn {
                            state: EvConnectionState::HomePluggedIn
                        }
                    )
                });
                assert!(!has_plugin, "should NOT plug in when SOC above threshold");
                assert_eq!(actor.phase, DriverPhase::HomePluggedIn);
                assert!(
                    (actor.estimated_soc - 0.82).abs() < 1e-6,
                    "estimated_soc should be reconciled to actual 0.82 even without plug-in, got {}",
                    actor.estimated_soc
                );
            }
        }
    }

    // ======= Range anxiety day-specific tests (T-0430) =======

    /// Helper: build an actor with specific drive parameters and directly set
    /// todays_event with the given drive_kwh so the day-specific anxiety
    /// calculation is exercised without a full drive cycle.
    fn make_anxiety_actor(
        expected_daily_miles: f64,
        fuel_economy_kwh_per_mi: f64,
        capacity_kwh: f64,
        range_anxiety_miles: f64,
        estimated_soc: f64,
        todays_drive_kwh: Option<f64>,
    ) -> EvDriverActor {
        let mut actor = EvDriverActor::new(
            "AnxietyDriver",
            "EV1",
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            ScheduleSource::Constant(expected_daily_miles),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(600.0),
            None,
            if todays_drive_kwh.is_some() { 1.0 } else { 0.0 },
            fuel_economy_kwh_per_mi,
            capacity_kwh,
            7.2,
            30.0,
            range_anxiety_miles,
            0.0,
            0.0,
            seed_from_u64(42),
        );
        actor.estimated_soc = estimated_soc;
        actor.phase = DriverPhase::HomePluggedIn;
        if let Some(drive_kwh) = todays_drive_kwh {
            actor.todays_event = Some(DayEvent {
                departure_minute: 480,
                arrival_minute: 1080,
                drive_kwh,
            });
        }
        actor
    }

    #[test]
    fn range_anxiety_uses_day_specific_drive_on_high_mileage_day() {
        // LongCommuterL2-like parameters: mean=75 mi, fuel=0.251 kWh/mi,
        // capacity=65 kWh, range_anxiety=20 mi.
        // Day-specific drive_kwh = 115 mi * 0.251 = 28.865 kWh (2-sigma high).
        // Static-mean anxiety: (75+20)*0.251*1.11/65 ≈ 0.407
        // Day-specific anxiety: (115+20)*0.251*1.11/65 ≈ 0.579
        // SOC at 0.45 should NOT trigger with static mean but SHOULD trigger
        // with day-specific value. Since we use max(day, mean), the threshold
        // is 0.579 and 0.45 < 0.579 → true.
        let actor = make_anxiety_actor(
            75.0,                // expected_daily_miles
            0.251,               // fuel_economy_kwh_per_mi
            65.0,                // capacity_kwh
            20.0,                // range_anxiety_miles
            0.45,                // estimated_soc
            Some(115.0 * 0.251), // todays_drive_kwh: 115 mi at 0.251 kWh/mi
        );

        let env = env_at_minute(0);
        assert!(
            actor.needs_range_anxiety_override(&env),
            "SOC 0.45 should trigger range anxiety on 115-mi day (threshold ~0.579), \
             but static-mean-only would be ~0.407 and miss it"
        );
    }

    #[test]
    fn range_anxiety_threshold_never_below_static_mean() {
        // On a below-average day, the threshold must not drop below the
        // static-mean baseline — the blended `max` approach ensures this.
        // Actor: mean=30 mi, fuel=0.3 kWh/mi, cap=60 kWh, anxiety=20 mi.
        // Static-mean anxiety_soc: (30+20)*0.3*1.11/60 ≈ 0.278
        // Day with only 5 mi → drive_kwh=1.5. Day-specific-only would give
        // (5+20)*0.3*1.11/60 ≈ 0.139 — too low, anxiety wouldn't fire
        // when it should. With max(5, 30) = 30, threshold stays at 0.278.
        let actor = make_anxiety_actor(
            30.0,      // expected_daily_miles
            0.3,       // fuel_economy_kwh_per_mi
            60.0,      // capacity_kwh
            20.0,      // range_anxiety_miles
            0.25,      // estimated_soc (below static-mean threshold 0.278)
            Some(1.5), // todays_drive_kwh: 5 mi * 0.3 kWh/mi
        );

        let env = env_at_minute(0);
        assert!(
            actor.needs_range_anxiety_override(&env),
            "SOC 0.25 should trigger range anxiety even on a below-average day \
             because blended max(5, 30) uses static-mean baseline 30 mi → threshold ~0.278"
        );
    }

    #[test]
    fn long_commuter_l2_115_mile_day_soc_0_45_triggers_range_anxiety() {
        // Regression test: LongCommuterL2 archetype with daily_drive_miles_mean=75.0,
        // daily_drive_miles_stddev=20.0. A 2-sigma draw of ~115 miles produces
        // drive_kwh = 115 * 0.251 = 28.865 kWh. The vehicle (Chevy Bolt EV: 65 kWh,
        // 0.251 kWh/mi) needs ~0.493 SOC for the trip.
        // Static-mean threshold = (75+20)*0.251*1.11/65 ≈ 0.407.
        // Day-specific threshold = (115+20)*0.251*1.11/65 ≈ 0.579.
        // At SOC=0.45: static-mean says "no anxiety" (0.45 > 0.407) — driver stranded.
        // Day-specific says "anxiety" (0.45 < 0.579) — driver saved.

        // Verify with the actual actor that 0.45 triggers with day-specific miles.
        let actor = make_anxiety_actor(
            75.0,                // expected_daily_miles (from daily_drive_miles_mean)
            0.251,               // fuel_economy_kwh_per_mi (Chevy Bolt EV)
            65.0,                // capacity_kwh (Chevy Bolt EV)
            20.0,                // range_anxiety_miles (default)
            0.45,                // SOC at 0.45
            Some(115.0 * 0.251), // drive_kwh for 115 mi at 0.251 kWh/mi
        );

        let env = env_at_minute(0);
        let ambient_c = env.weather.outdoor_temp_c;
        let temp_mult = temp_efficiency_multiplier(ambient_c);

        // Compute expected thresholds for documentation and verification.
        let static_mean_threshold = (75.0 + 20.0) * 0.251 * temp_mult / 65.0;
        let day_specific_threshold = (115.0 + 20.0) * 0.251 * temp_mult / 65.0;

        // The static-mean-only threshold must be below 0.45 (proving the old code
        // would fail to protect the driver).
        assert!(
            static_mean_threshold < 0.45,
            "static-mean threshold {static_mean_threshold:.3} should be below SOC 0.45 \
             (otherwise the bug scenario is not reproducible)"
        );

        // The day-specific threshold must be above 0.45 (proving the fix works).
        assert!(
            day_specific_threshold > 0.45,
            "day-specific threshold {day_specific_threshold:.3} should be above SOC 0.45 \
             (the fix must catch this case)"
        );

        assert!(
            actor.needs_range_anxiety_override(&env),
            "LongCommuterL2 with 115-mi day at SOC 0.45 must trigger range anxiety; \
             static-mean threshold={static_mean_threshold:.3}, \
             day-specific threshold={day_specific_threshold:.3}"
        );
    }

    #[test]
    fn multi_day_low_soc_no_soc_drift_at_arrival() {
        use chrono::TimeZone;
        // 8-day simulation with PlugInPolicy::LowSoc against a *moving* ground
        // truth: the equipment BMS SOC is set to a distinct value each day,
        // simulating overnight grid charging and away-charging restoring the
        // battery to a different level than the driver's decremented estimate.
        // At each arrival reconciliation, estimated_soc must snap to that day's
        // actual SOC — proving reconciliation tracks the target it is reconciling
        // against, not merely re-writing a single value it saw once. A static
        // ground truth would pass even if reconciliation were deleted; this one
        // cannot, because estimated_soc is driven away from the new actual by
        // driving-decrement each day and only the arrival reconciliation can
        // bring it back to the day's true value.
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::LowSoc { threshold: 0.5 },
            42,
        );
        let mut env = env_at_minute(0);

        // Distinct actual SOC per day. Values differ from each other and from
        // the driving-decremented estimate so a stale estimate is detectable.
        let daily_actual_soc = [0.85, 0.62, 0.91, 0.55, 0.78, 0.48, 0.88, 0.66];

        let mut out = Vec::new();
        let mut arrival_reconciliation_count = 0usize;
        let mut observed_actuals: Vec<f64> = Vec::new();

        for (day, &day_soc) in daily_actual_soc.iter().enumerate() {
            let day = day as u32;

            // Simulate the BMS having charged/drained the battery overnight to a
            // new level before the driver departs for the day. Set it at the top
            // of the day so driving-decrement then drives estimated_soc away from
            // it, leaving a genuine gap for arrival reconciliation to close.
            set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), day_soc);

            for minute in 0_u16..1440 {
                let abs_minute = day * 1440 + minute as u32;
                let h = (abs_minute / 60).min(23) as u8;
                let m = (abs_minute % 60) as u8;
                let tz = chrono::FixedOffset::east_opt(0).unwrap();
                env.current_time = tz
                    .with_ymd_and_hms(2026, 1, 1 + day, h as u32, m as u32, 0)
                    .single()
                    .unwrap();

                out.clear();
                actor.decide(&env, &mut out);

                // At the arrival minute, reconciliation has just fired.
                // Verify estimated_soc matches this day's actual equipment SOC.
                if matches!(actor.phase, DriverPhase::HomePluggedIn) && minute == 1080 {
                    if let Some(actual) = actor.actual_soc(&env) {
                        arrival_reconciliation_count += 1;
                        observed_actuals.push(actual);
                        let drift = (actor.estimated_soc - actual).abs();
                        // Ticket tolerance is <1% drift; reconciliation is exact,
                        // so 1e-6 is the meaningful bound.
                        assert!(
                            drift < 1e-6,
                            "day {day}: after arrival reconciliation, estimated_soc ({}) should equal this day's actual SOC ({actual})",
                            actor.estimated_soc
                        );
                    }
                }
            }
        }

        assert!(
            arrival_reconciliation_count > 0,
            "should have observed at least one arrival reconciliation"
        );

        // Guard against the static-ground-truth regression: the test must have
        // reconciled against more than one distinct actual value, otherwise it
        // could not distinguish "tracks a moving target" from "writes one value".
        let distinct = observed_actuals
            .iter()
            .fold(Vec::<f64>::new(), |mut acc, &v| {
                if !acc.iter().any(|&a| (a - v).abs() < 1e-6) {
                    acc.push(v);
                }
                acc
            });
        assert!(
            distinct.len() > 1,
            "test must reconcile against a varying ground truth to be meaningful, saw {} distinct values",
            distinct.len()
        );
    }

    /// A configured LowSoc gate must hold when SOC is above the gate. At SOC
    /// 0.20 with a 0.15 threshold the strategy says "do not charge", so no
    /// charging signal may be dispatched. This probes the band where the
    /// range-anxiety override (fixed 30 mi/day + 20 mi buffer at 0.3 kWh/mi on
    /// a 60 kWh pack => anxiety_soc 0.25) currently preempts the composer
    /// unconditionally, collapsing every strategy to charge-to-full.
    #[test]
    fn range_anxiety_override_does_not_preempt_low_soc_gate() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::LowSoc {
                threshold: 0.15,
                target_soc: 0.9,
            },
            0.20, // above the LowSoc gate: the strategy itself should idle
        );
        let out = plugged_in_step(&mut actor, 19 * 60);

        let soc_target = out.iter().find_map(|r| match &r.signal {
            ControlSignal::SOCTarget { target_soc, .. } => Some(*target_soc),
            _ => None,
        });
        assert!(
            soc_target.is_none(),
            "SOC 0.20 is above LowSoc's configured 0.15 threshold, so the strategy gate \
             says idle — but a charging signal (SOCTarget {soc_target:?}) was dispatched, \
             meaning the range-anxiety override preempted the configured strategy"
        );
    }

    /// A configured Nightly window must hold outside the window. At 19:00 a
    /// Nightly{22:00-06:00} strategy says "wait", so no charging signal may be
    /// dispatched — yet the range-anxiety override fires for the same SOC band
    /// and dispatches charge-to-full, making Nightly indistinguishable from
    /// Immediate for any EV whose driving load enters that band.
    #[test]
    fn range_anxiety_override_does_not_preempt_nightly_window() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.95,
            },
            0.20,
        );
        let out = plugged_in_step(&mut actor, 19 * 60); // outside the 22:00-06:00 window

        let soc_target = out.iter().find_map(|r| match &r.signal {
            ControlSignal::SOCTarget { target_soc, .. } => Some(*target_soc),
            _ => None,
        });
        assert!(
            soc_target.is_none(),
            "19:00 is outside Nightly's configured 22:00-06:00 window, so the strategy \
             says wait — but a charging signal (SOCTarget {soc_target:?}) was dispatched, \
             meaning the range-anxiety override preempted the configured strategy"
        );
    }

    /// Builds the equipment half of an actor/equipment pair the way an HPXML
    /// `Equipment["EV"]` override leaves it: `charging_strategy` (when set)
    /// reaches the actor via `Ev::actor_seed()`, but `ready_soc` is never set
    /// (no strategy variant populates it), so the equipment retains its BMS
    /// default of charging toward soc_max whenever it is plugged in with no
    /// actor instruction.
    fn make_hpxml_override_ev(
        initial_soc: f64,
        strategy: Option<ChargingStrategy>,
    ) -> hares_equipment::ev::Ev {
        use hares_equipment::Equipment;
        use hares_equipment::config::EquipmentConfig;
        use hares_equipment::ev::EvConfig;
        // Deliberately no `ready_soc`: the ticket's Equipment["EV"] override
        // sets only `charging_strategy`, which is actor-side (fed through
        // `Ev::actor_seed()`) and never reaches the equipment's own config.
        let config = EquipmentConfig::from_typed(
            "EV1".to_string(),
            "EV".to_string(),
            EvConfig {
                equipment_id: None,
                capacity_kwh: 60.0,
                charging_level: None,
                max_charging_power_kw: 7.2,
                charging_efficiency: None,
                l1_current_a: None,
                l1_voltage_v: None,
                soc_max: None,
                initial_soc: Some(initial_soc),
                battery_temp_c: None,
                min_charge_temp_c: None,
                full_power_temp_c: None,
                heater_power_w: None,
                heater_threshold_c: None,
                thermal_mass_j_per_k: None,
                ua_w_per_k: None,
                n_series: None,
                n_parallel: None,
                cell_resistance_ohm: None,
                v2l_enabled: None,
                v2l_soc_reserve: None,
                v2l_max_discharge_kw: None,
                v2g_enabled: None,
                v2g_soc_reserve: None,
                v2g_max_discharge_kw: None,
                chemistry: None,
                fuel_economy_kwh_per_mi: None,
                ready_soc: None,
                // The override carries the strategy as its serialized form —
                // the same JSON string an HPXML `Equipment["EV"]` merge
                // puts in the raw config, which `init_typed` parses back
                // through `parse_charging_strategy`.
                charging_strategy: strategy
                    .map(|s| serde_json::to_string(&s).expect("ChargingStrategy serializes")),
                plug_in_policy: None,
                power_limit_kw: None,
                initial_connection_state: None,
                power_factor: None,
                charger_capacity_kva: None,
                cc_cv_transition_soc: None,
                charging_priority: None,
                discharge_respects_deadline: true,
            },
        )
        .expect("typed EV config");
        let mut ev = hares_equipment::ev::Ev::new(config.clone());
        ev.init(&config, &env_at_minute(18 * 60))
            .expect("EV init with valid config");
        ev
    }

    /// Steps an actor and a real equipment instance together through
    /// [start_minute, end_minute) at 5-minute resolution, applying every
    /// dispatch the actor emits to the equipment, and returns the equipment's
    /// SOC afterwards. This is the composition no existing test exercises:
    /// what the equipment actually does while a configured strategy idles.
    fn run_actor_equipment_window(
        strategy: ChargingStrategy,
        start_minute: u16,
        end_minute: u16,
    ) -> f64 {
        use hares_equipment::Equipment;
        let mut actor = make_plugged_in_actor(strategy, 0.5);
        let mut ev = make_hpxml_override_ev(0.5, None);
        let mut minute = start_minute;
        while minute < end_minute {
            let env = env_at_minute(minute);
            let mut out = Vec::new();
            actor.decide(&env, &mut out);
            for request in &out {
                ev.apply_control_unchecked(&request.signal)
                    .expect("actor dispatch must be valid for the EV");
            }
            let mut ports = hares_types::PortSlots::default();
            ev.step(&env, std::time::Duration::from_secs(300), &mut ports)
                .expect("EV step");
            minute += 5;
        }
        ev.core_output()
            .state
            .soc
            .expect("EV core output always reports SOC")
            .get()
    }

    /// A Nightly strategy outside its off-peak window must not charge the
    /// vehicle. Plugged in at 18:00 with the window opening at 22:00, the
    /// strategy says "wait" for the whole 18:00-22:00 span, so the EV must
    /// hold its plug-in SOC (0.5) through it. Today the actor dispatches
    /// nothing when the strategy idles, and the equipment's BMS default
    /// charges to full on every silent step — so SOC climbs through the
    /// entire idle window, making Nightly indistinguishable from Immediate.
    #[test]
    fn idle_nightly_window_does_not_charge_equipment() {
        let soc_after = run_actor_equipment_window(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.95,
            },
            18 * 60,
            22 * 60,
        );
        assert!(
            soc_after <= 0.51,
            "Nightly 22:00-06:00 is idle from 18:00 to 22:00, so the EV must hold its \
             plug-in SOC (0.5) through that window — instead it charged to {soc_after:.3}, \
             meaning the actor's idle decision was silently overridden by the equipment's \
             charge-to-full BMS default"
        );
    }

    /// The same invariant for the SOC-gated policy class: LowSoc{0.15} at SOC
    /// 0.5 is above its gate, so the strategy says "wait" — no charging may
    /// occur. Today actor silence during the hold is filled by the
    /// equipment's charge-to-full default, so LowSoc charges identically to
    /// every other strategy.
    #[test]
    fn idle_low_soc_gate_does_not_charge_equipment() {
        let soc_after = run_actor_equipment_window(
            ChargingStrategy::LowSoc {
                threshold: 0.15,
                target_soc: 0.9,
            },
            18 * 60,
            22 * 60,
        );
        assert!(
            soc_after <= 0.51,
            "SOC 0.5 is above LowSoc's configured 0.15 gate, so the EV must hold its \
             plug-in SOC (0.5) while the gate holds — instead it charged to {soc_after:.3}, \
             meaning the actor's idle decision was silently overridden by the equipment's \
             charge-to-full BMS default"
        );
    }

    /// The arrival step itself must carry a charging decision: the plug-in
    /// dispatch is applied before the equipment steps this timestep, so if
    /// the strategy's decision waited until the *next* step the EV would
    /// charge one full step at rated power toward whatever setpoint was last
    /// latched, outside whatever window the strategy configures. With a
    /// Nightly 22:00-06:00 strategy and an 18:00 arrival, the arrival step
    /// must dispatch the explicit hold alongside the plug-in — and the
    /// plug-in must precede it, because the dwelling applies same-tier
    /// dispatches FIFO and the hold is only meaningful once the vehicle is
    /// connected.
    #[test]
    fn arrival_step_dispatches_strategy_hold_not_a_step_later() {
        let mut actor = make_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            PlugInPolicy::Always,
            42,
        );
        let mut out = Vec::new();

        // Roll today's event, depart at 08:00, drive through to 17:59.
        actor.decide(&env_at_minute(0), &mut out);
        out.clear();
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();
        for step in 1..=119 {
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
            out.clear();
        }

        // The 18:00 arrival step: 10 h away ends and the vehicle plugs in.
        // Post-drive SOC (~0.85 from a 30 mi day) is far above the anxiety
        // band, so nothing but the strategy's own decision can speak here.
        out.clear();
        actor.decide(&env_at_minute(18 * 60), &mut out);

        let plug_in_pos = out.iter().position(|d| {
            matches!(
                d.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn
                }
            )
        });
        let hold_pos = out.iter().position(is_explicit_hold);
        assert!(
            plug_in_pos.is_some(),
            "arrival at 18:00 must dispatch the plug-in, got: {out:?}"
        );
        assert!(
            hold_pos.is_some(),
            "the arrival step itself must dispatch the strategy's decision — a hold outside \
             the 22:00-06:00 window — not leave the equipment to one ungoverned step, got: {out:?}"
        );
        assert!(
            plug_in_pos.unwrap() < hold_pos.unwrap(),
            "the plug-in must precede the hold so the connection transition is in effect \
             before the charging decision is applied, got: {out:?}"
        );
        assert!(
            !out.iter().any(commands_charging),
            "outside the window the arrival step must not command charging, got: {out:?}"
        );
    }

    // ======= Actor/equipment integration: the ticket's completion bar =======

    /// Test env at an exact minute of a given day (1-based) with a chosen
    /// outdoor temperature. The fixed-date helpers delegate here.
    fn env_at_day_minute_temp(day: u32, minute: u16, outdoor_temp_c: f64) -> EnvironmentState {
        let hour = (minute / 60).min(23) as u8;
        let min = minute % 60;
        use chrono::{FixedOffset, TimeZone};
        let tz = FixedOffset::east_opt(0).unwrap();
        let mut env = test_env()
            .hour(hour)
            .date(2026, 1, day)
            .outdoor_temp(outdoor_temp_c)
            .build();
        env.current_time = tz
            .with_ymd_and_hms(2026, 1, day, hour as u32, min as u32, 0)
            .single()
            .unwrap();
        env
    }

    /// Build a seeded actor/equipment pair through the real Dwelling
    /// construction path: the equipment's `EvConfig` carries the strategy
    /// (exactly what an HPXML `Equipment["EV"]` override produces),
    /// `actor_seed()` derives `ActorSeed::Ev` from it, and
    /// `build_actors_from_seeds` builds the driver with the same hardcoded
    /// driving parameters a Dwelling uses. The RNG is fixed so every run
    /// rolls the identical driving-day pattern.
    fn make_seeded_ev_driver(
        strategy: ChargingStrategy,
        initial_soc: f64,
    ) -> (Box<dyn crate::Actor>, Box<dyn hares_equipment::Equipment>) {
        use hares_equipment::Equipment as _;
        use std::collections::HashMap;
        let ev = make_hpxml_override_ev(initial_soc, Some(strategy));
        let name = ev.descriptor().name.clone();
        let equipment: Box<dyn hares_equipment::Equipment> = Box::new(ev);
        let mut id_by_name = HashMap::new();
        id_by_name.insert(name, EquipmentId(1));
        let rng = seed_from_u64(42);
        let mut actors = crate::dwelling::build_actors_from_seeds(
            std::slice::from_ref(&equipment),
            &[],
            false, // no tariff — none of the strategies under test needs one
            None,
            96,
            &id_by_name,
            &rng,
        );
        assert_eq!(
            actors.len(),
            1,
            "the seeded EV must produce exactly one driver actor"
        );
        (actors.pop().expect("exactly one actor"), equipment)
    }

    /// Step a seeded actor/equipment pair through `days` simulated days at
    /// 15-minute resolution, mirroring the Dwelling's per-step order: the
    /// actor decides against the previous step's committed equipment
    /// output, dispatches apply FIFO, then the equipment steps. Returns
    /// per-day hourly charging energy (kWh, home charging only — driving
    /// and away steps draw zero port power) and the indices of days on
    /// which a drive occurred.
    fn run_seeded_profile(
        strategy: ChargingStrategy,
        initial_soc: f64,
        days: usize,
    ) -> (Vec<[f64; 24]>, Vec<usize>) {
        let (mut actor, mut equipment) = make_seeded_ev_driver(strategy, initial_soc);
        let equipment_id = EquipmentId(1);

        let mut hourly: Vec<[f64; 24]> = vec![[0.0; 24]; days];
        let mut driving_days = Vec::new();
        let mut core = CoreOutput::default();
        let mut out = Vec::new();

        for (day, day_hours) in hourly.iter_mut().enumerate() {
            let mut drove_today = false;
            for quarter in 0..96u16 {
                let minute = quarter * 15;
                let hour = (minute / 60) as usize;
                let mut env = env_at_day_minute_temp(day as u32 + 1, minute, 10.0);
                env.time_res = chrono::Duration::minutes(15);
                env.equipment_core.insert(equipment_id, core.clone());

                out.clear();
                actor.decide(&env, &mut out);
                for req in &out {
                    drove_today |= matches!(req.signal, ControlSignal::EvDrive { .. });
                    equipment
                        .apply_control_unchecked(&req.signal)
                        .expect("driver dispatch must be valid for the EV");
                }

                let mut ports = hares_types::PortSlots::default();
                equipment
                    .step(&env, std::time::Duration::from_secs(900), &mut ports)
                    .expect("EV step");
                core = equipment.core_output().clone();
                let power_kw = core.flows.electric_kw.map(|p| p.signed_kw()).unwrap_or(0.0);
                if power_kw > 0.0 {
                    day_hours[hour] += power_kw * 0.25;
                }
            }
            if drove_today {
                driving_days.push(day);
            }
        }
        (hourly, driving_days)
    }

    /// Total charging energy in the given hours across all days.
    fn energy_in_hours(profile: &[[f64; 24]], hours: &[usize]) -> f64 {
        profile
            .iter()
            .map(|day| hours.iter().map(|h| day[*h]).sum::<f64>())
            .sum()
    }

    /// First hour with non-zero charging energy after the 18:00 plug-in on
    /// `day`, scanning that evening (18:00–24:00) and the following
    /// morning (00:00–06:00) — a window starting after midnight (e.g.
    /// 02:00) charges in the next calendar day's early hours. `None` means
    /// no charging in that span (unobservable when `day` is the last
    /// simulated day).
    fn first_charging_hour_after_plugin(profile: &[[f64; 24]], day: usize) -> Option<usize> {
        if let Some(h) = (18..24).find(|&h| profile[day][h] > 0.0) {
            return Some(h);
        }
        profile
            .get(day + 1)
            .and_then(|next| (0..6).find(|&h| next[h] > 0.0))
    }

    /// The ticket's completion bar, part 1: two `Nightly` overrides
    /// differing only in `off_peak_start_hour` must produce measurably
    /// different hourly charging profiles — each run charges only inside
    /// its own configured window, and the first charging hour after the
    /// 18:00 plug-in shifts with the window start.
    ///
    /// Precondition (computed, not observed): the range-anxiety override
    /// provably never fires. `anxiety_soc` = (30 mi + 20 mi buffer) ×
    /// 0.3 kWh/mi × temp_mult(10 °C) / 60 kWh ≈ 0.278, and the vehicle never
    /// drops below ~0.73 SOC (0.95 nightly target − 0.167 daily drain), so
    /// perceived SOC never enters the band.
    #[test]
    fn nightly_start_hour_governs_hourly_charging_profile() {
        let days = 12;
        let nightly_at = |start: f64| ChargingStrategy::Nightly {
            off_peak_start_hour: start,
            off_peak_end_hour: 6.0,
            target_soc: 0.95,
        };
        let (profile_a, drives_a) = run_seeded_profile(nightly_at(22.0), 0.9, days);
        let (profile_b, drives_b) = run_seeded_profile(nightly_at(2.0), 0.9, days);

        assert!(!drives_a.is_empty(), "fixture must contain driving days");
        assert_eq!(
            drives_a, drives_b,
            "same RNG seed must roll the identical driving-day pattern for both runs"
        );

        // Well under one 15-minute charging step at rated power (1.8 kWh):
        // numerical noise, not a charging event.
        let eps = 0.05;
        let in_window_a = |h: usize| !(6..22).contains(&h); // 22:00–06:00, wrapping midnight
        let in_window_b = |h: usize| (2..6).contains(&h);

        // (a) Charging energy outside each run's own window is zero, every
        // simulated day — the strategy's window, not the BMS default,
        // governs when the vehicle charges.
        for (day, hours) in profile_a.iter().enumerate() {
            let outside: f64 = hours
                .iter()
                .enumerate()
                .filter(|(h, _)| !in_window_a(*h))
                .map(|(_, e)| e)
                .sum();
            assert!(
                outside < eps,
                "Nightly 22:00–06:00, day {day}: {outside:.3} kWh charged \
                 outside the configured window"
            );
        }
        for (day, hours) in profile_b.iter().enumerate() {
            let outside: f64 = hours
                .iter()
                .enumerate()
                .filter(|(h, _)| !in_window_b(*h))
                .map(|(_, e)| e)
                .sum();
            assert!(
                outside < eps,
                "Nightly 02:00–06:00, day {day}: {outside:.3} kWh charged \
                 outside the configured window"
            );
        }

        // (b) The two windows produce measurably different profiles: the
        // 22:00 run charges in the late evening and not the early morning;
        // the 02:00 run the reverse.
        assert!(
            energy_in_hours(&profile_a, &[22, 23]) > 1.0,
            "the 22:00 run must charge in hours 22–23, got {:?}",
            profile_a.iter().map(|d| (d[22], d[23])).collect::<Vec<_>>()
        );
        assert!(
            energy_in_hours(&profile_b, &[2, 3, 4, 5]) > 1.0,
            "the 02:00 run must charge in hours 02–05, got {:?}",
            profile_b
                .iter()
                .map(|d| (d[2], d[3], d[4], d[5]))
                .collect::<Vec<_>>()
        );
        assert!(
            energy_in_hours(&profile_a, &[2, 3, 4, 5]) < eps,
            "the 22:00 run must not charge in the 02:00 run's morning hours"
        );
        assert!(
            energy_in_hours(&profile_b, &[22, 23]) < eps,
            "the 02:00 run must not charge in the 22:00 run's evening hours"
        );

        // (b') On every driving day the first charging hour after the
        // 18:00 plug-in is each run's own window start — a four-hour shift
        // in the configuration produces a four-hour shift in the profile.
        // Days without a simulated successor are excluded: a window that
        // opens after midnight charges in the next calendar day's hours,
        // which do not exist for the final day.
        for &day in drives_a.iter().filter(|&&d| d + 1 < days) {
            assert_eq!(
                first_charging_hour_after_plugin(&profile_a, day),
                Some(22),
                "22:00 run, driving day {day}: first charging hour after plug-in"
            );
        }
        for &day in drives_b.iter().filter(|&&d| d + 1 < days) {
            assert_eq!(
                first_charging_hour_after_plugin(&profile_b, day),
                Some(2),
                "02:00 run, driving day {day}: first charging hour after plug-in"
            );
        }
    }

    /// The ticket's completion bar, part 2: `LowSoc` differs from
    /// `Immediate` — a different policy class (charge when SOC falls below
    /// a threshold, versus charge on plug-in).
    ///
    /// Precondition (computed, not observed): the threshold is chosen so
    /// the range-anxiety override provably never fires and the gate alone
    /// governs, with margin on both sides of the gate. `anxiety_soc` ≈ 0.28
    /// at 10 °C ((30 mi + 20 mi) × 0.325 kWh/mi × 1.11 / 60 kWh); the
    /// one-day drain is ≈ 0.175 SOC (30 mi × 0.325 kWh/mi × 1.11 / 60 kWh).
    /// LowSoc 0.65 with the 0.05 hysteresis band recharges at 0.60: the
    /// first driving day's arrival SOC (0.9 − 0.175 ≈ 0.725) sits above
    /// the 0.65 upper threshold (block, margin ≈ 0.075) and the second's
    /// (≈ 0.550) below the 0.60 lower threshold (open, margin ≈ 0.05), so
    /// one day's drive can never carry SOC from above the gate into the
    /// anxiety band (0.60 ≫ 0.28 + 0.175). (The ticket's literal
    /// `LowSoc 0.15` sits below the hardcoded anxiety band and remains
    /// governed by the override — docs/tickets/132.)
    #[test]
    fn low_soc_gate_delays_charging_beyond_immediate() {
        let days = 12;
        let (immediate, drives_imm) =
            run_seeded_profile(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.9, days);
        let (low_soc, drives_low) = run_seeded_profile(
            ChargingStrategy::LowSoc {
                threshold: 0.65,
                target_soc: 0.9,
            },
            0.9,
            days,
        );

        assert!(
            drives_low.len() >= 3,
            "fixture precondition: at least 3 driving days in {days} days at \
             event_day_ratio 0.8 (got {})",
            drives_low.len()
        );
        assert_eq!(drives_imm, drives_low, "same RNG seed, same day pattern");

        // Immediate: non-zero charging energy in the first hour after
        // plug-in on the first driving day — charge-on-plug-in.
        let first_drive = drives_imm[0];
        assert!(
            immediate[first_drive][18] > 0.0,
            "Immediate must charge in the first hour after plug-in (driving day \
             {first_drive}, hour 18), got {}",
            immediate[first_drive][18]
        );

        // LowSoc: zero charging energy on the first driving day — the
        // arrival SOC (≈ 0.725) is above the 0.65 gate, so the strategy
        // holds instead of charging. Exactly the difference in policy
        // class the ticket asks to be observable.
        let first_day_total: f64 = low_soc[first_drive].iter().sum();
        assert!(
            first_day_total < 1e-9,
            "LowSoc must not charge on the first driving day while SOC (≈ 0.725) is \
             above its gate, got {first_day_total:.3} kWh"
        );

        // …and it charges on the second driving day, once the drain carries
        // SOC below the gate's lower threshold (≈ 0.550 < 0.60) — beginning
        // in the plug-in hour, gate-governed, not window-governed.
        let second_drive = drives_low[1];
        assert!(
            low_soc[second_drive][18] > 0.0,
            "LowSoc must charge in the plug-in hour of the second driving day, once \
             SOC (≈ 0.550) has crossed below the gate (driving day {second_drive}), \
             got {}",
            low_soc[second_drive][18]
        );

        // Class-level contrast over the whole run: Immediate charges on
        // every driving day; LowSoc rests on the days the gate holds, so it
        // charges on strictly fewer days.
        let charging_days = |profile: &[[f64; 24]]| {
            profile
                .iter()
                .filter(|day| day.iter().any(|&e| e > 0.0))
                .count()
        };
        let imm_days = charging_days(&immediate);
        let low_days = charging_days(&low_soc);
        assert!(
            low_days < imm_days,
            "LowSoc must charge on fewer days than Immediate (gate-governed rest \
             days), got {low_days} vs {imm_days}"
        );
    }

    // ======= Red-team: soc-floor override vs. the explicit-hold contract =======

    /// A V2G soc-floor override must not deadlock the stack's own recovery.
    /// At or below `min_soc`, `V2GExport` short-circuits the composer with
    /// an idle override on every step, and under the explicit-hold contract
    /// that idle dispatches a latching zero-power hold. The hold's only
    /// release paths — a fresh `SOCTarget`/`EvSetReadyBy`, or disconnect —
    /// never fire while the floor keeps short-circuiting, and only charging
    /// raises SOC, so the hold pins the vehicle at the floor indefinitely
    /// and the stack's own `SocTarget{1.0}` (the only vote in the stack
    /// that ever wants to charge) is never consulted. The floor's meaning
    /// is "do not export below this SOC" (the equipment enforces exactly
    /// that in `compute_v2g_discharge`); before the explicit-hold contract
    /// the idle override dispatched silence, which the equipment's BMS
    /// default filled by recharging the vehicle — the floor path's only
    /// recovery mechanism.
    #[test]
    fn v2g_soc_floor_override_does_not_deadlock_stack_target_recovery() {
        let env = test_env().hour(19).build();
        let ctx = DecisionContext {
            current_soc: 0.25, // below the floor (0.3)
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env: &env,
            current_minute: 19 * 60,
            next_departure_minute: Some(480),
            time_res_minutes: 5.0,
            observed_charge_derate: None,
        };

        let strategy = ChargingStrategy::V2G {
            min_soc: 0.3,
            max_export_kw: 5.0,
            price_threshold: 0.20,
        };
        let mut composer =
            ChargingComposer::new(build_preferences(&strategy, 7.2, 0.9, None, 288), "EV1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert!(
            out.iter().any(commands_charging),
            "V2G: a vehicle resting at/below its soc floor must still be able to recharge \
             through its own SocTarget — the floor bans export, not charging. Instead the \
             floor override's idle vote dispatched only a hold, which nothing ever lifts \
             while the floor keeps short-circuiting the composer, got {out:?}"
        );
    }

    /// The same deadlock for the V2H stack (`V2HDischarge`'s soc-floor
    /// override short-circuiting `SocTarget{0.9}`): the floor bans
    /// discharging the home below `min_soc`, not recharging the vehicle.
    #[test]
    fn v2h_soc_floor_override_does_not_deadlock_stack_target_recovery() {
        let env = test_env().hour(19).build();
        let ctx = DecisionContext {
            current_soc: 0.25, // below the floor (0.3)
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env: &env,
            current_minute: 19 * 60,
            next_departure_minute: Some(480),
            time_res_minutes: 5.0,
            observed_charge_derate: None,
        };

        let strategy = ChargingStrategy::V2H {
            discharge_threshold_soc: 0.5,
            min_soc: 0.3,
        };
        let mut composer =
            ChargingComposer::new(build_preferences(&strategy, 7.2, 0.9, None, 288), "EV1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert!(
            out.iter().any(commands_charging),
            "V2H: a vehicle resting at/below its soc floor must still be able to recharge \
             through its own SocTarget — the floor bans discharging to the home, not \
             charging. Instead the floor override's idle vote dispatched only a hold, \
             which nothing ever lifts while the floor keeps short-circuiting the \
             composer, got {out:?}"
        );
    }

    /// The physical consequence of the soc-floor hold, end to end: a V2G
    /// vehicle that landed below its floor must recharge, not sit there.
    /// Perceived SOC 0.25 is inside the anxiety band (~0.278 at 10 °C) but
    /// the next departure (08:00, wrapped to tomorrow — 13 h away) gives the
    /// urgency gate every reason to stand the override down, so the strategy
    /// itself governs. Stepping the pair from 19:00 to 23:00, the vehicle
    /// must climb back above its 0.3 floor; a hold leaves it frozen at the
    /// plug-in SOC.
    #[test]
    fn v2g_vehicle_below_soc_floor_recovers_overnight() {
        use hares_equipment::Equipment as _;

        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2G {
                min_soc: 0.3,
                max_export_kw: 5.0,
                price_threshold: 0.20,
            },
            0.25,
        );
        let mut ev = make_hpxml_override_ev(0.25, None);

        let mut minute = 19 * 60;
        while minute < 23 * 60 {
            let env = env_at_minute(minute);
            let mut out = Vec::new();
            actor.decide(&env, &mut out);
            for request in &out {
                ev.apply_control_unchecked(&request.signal)
                    .expect("actor dispatch must be valid for the EV");
            }
            let mut ports = hares_types::PortSlots::default();
            ev.step(&env, std::time::Duration::from_secs(300), &mut ports)
                .expect("EV step");
            minute += 5;
        }

        let soc_after = ev
            .core_output()
            .state
            .soc
            .expect("EV core output always reports SOC")
            .get();
        assert!(
            soc_after > 0.3,
            "a V2G vehicle below its 0.3 soc floor must recharge (the floor bans export, \
             not charging; its own SocTarget{{1.0}} wants the pack full) — instead it sat \
             at {soc_after:.3} for four hours, pinned by the floor override's latched hold"
        );
    }

    /// With the floor no longer a composer override, the actor-side
    /// protection is gone the moment the driver's belief goes stale: during
    /// a plug-in session the actor never reconciles its perceived SOC, so a
    /// driver believing 0.9 keeps voting export every step while the real
    /// pack drains. The fix's own comment claims the equipment backstop
    /// covers this ("the equipment independently enforces it ... via the
    /// `min_soc` this preference places on its discharge setpoints") — the
    /// vote-carried floor must survive the resolve fold and the
    /// target-then-rate dispatch pair and stop the discharge at exactly the
    /// strategy's floor. The floor here (0.5) sits deliberately above the
    /// equipment's `v2g_soc_reserve` default (0.3): if the fold or either
    /// dispatch ever drops the vote's `min_soc`, the equipment silently
    /// falls back to the reserve and discharges straight through 0.5 —
    /// which this test catches.
    #[test]
    fn discharge_floor_holds_when_stale_belief_keeps_voting_export() {
        use hares_equipment::Equipment as _;
        use hares_equipment::config::EquipmentConfig;
        use hares_equipment::ev::EvConfig;

        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2G {
                min_soc: 0.5,
                max_export_kw: 5.0,
                price_threshold: 0.20,
            },
            0.9, // stale belief: far above floor and threshold; never reconciled
        );
        let config = EquipmentConfig::from_typed(
            "EV1".to_string(),
            "EV".to_string(),
            EvConfig {
                equipment_id: None,
                capacity_kwh: 60.0,
                charging_level: None,
                max_charging_power_kw: 7.2,
                charging_efficiency: None,
                l1_current_a: None,
                l1_voltage_v: None,
                soc_max: None,
                initial_soc: Some(0.55),
                battery_temp_c: None,
                min_charge_temp_c: None,
                full_power_temp_c: None,
                heater_power_w: None,
                heater_threshold_c: None,
                thermal_mass_j_per_k: None,
                ua_w_per_k: None,
                n_series: None,
                n_parallel: None,
                cell_resistance_ohm: None,
                v2l_enabled: None,
                v2l_soc_reserve: None,
                v2l_max_discharge_kw: None,
                v2g_enabled: Some(true),
                v2g_soc_reserve: None, // default 0.3 — strictly below the 0.5 floor
                v2g_max_discharge_kw: None,
                chemistry: None,
                fuel_economy_kwh_per_mi: None,
                ready_soc: None,
                charging_strategy: None,
                plug_in_policy: None,
                power_limit_kw: None,
                initial_connection_state: None,
                power_factor: None,
                charger_capacity_kva: None,
                cc_cv_transition_soc: None,
                charging_priority: None,
                discharge_respects_deadline: true,
            },
        )
        .expect("typed EV config");
        let mut ev = hares_equipment::ev::Ev::new(config.clone());
        ev.init(&config, &env_at_minute(19 * 60))
            .expect("EV init with valid config");

        let mut min_soc_observed = 1.0_f64;
        let mut minute = 19 * 60;
        while minute < 20 * 60 {
            let mut env = env_at_minute(minute);
            env.price_signal = PriceSignal {
                electricity_price: Some(0.50), // well above the 0.20 threshold
                ..Default::default()
            };
            let mut out = Vec::new();
            actor.decide(&env, &mut out);
            for request in &out {
                ev.apply_control_unchecked(&request.signal)
                    .expect("actor dispatch must be valid for the EV");
            }
            let mut ports = hares_types::PortSlots::default();
            ev.step(&env, std::time::Duration::from_secs(300), &mut ports)
                .expect("EV step");
            let soc = ev
                .core_output()
                .state
                .soc
                .expect("EV core output always reports SOC")
                .get();
            min_soc_observed = min_soc_observed.min(soc);
            minute += 5;
        }

        assert!(
            min_soc_observed < 0.55 - 1e-3,
            "the export votes must actually discharge the pack for this test to mean \
             anything — a rejected dispatch would freeze SOC and prove nothing, \
             min soc observed {min_soc_observed:.4}"
        );
        assert!(
            min_soc_observed >= 0.5 - 1e-6,
            "a stale driver belief (0.9) voting export every step must not discharge the \
             pack below the strategy's 0.5 soc floor — the vote-carried min_soc must reach \
             the equipment through the resolve fold and bound the discharge; instead the \
             pack sank to {min_soc_observed:.4} (the equipment's 0.3 reserve fallback is \
             the only explanation for a floor below 0.5)"
        );
    }

    /// The same stale-belief composition through the **V2L** leg: the
    /// `V2HDischarge` floor comment claims the equipment enforces the
    /// vote-carried floor in `compute_v2l_discharge`, but the fix's own V2L
    /// landing test passes `min_soc: None` (reserve only) and no test walks
    /// the vote's `min_soc` through the resolve fold and dispatch pair into
    /// the V2L fast path (`v2g_enabled` absent, `v2l_enabled` set). The
    /// floor (0.5) sits deliberately above the `v2l_soc_reserve` default
    /// (0.2): if the fold or either dispatch drops the vote's `min_soc`,
    /// the pack silently falls back to the reserve and discharges straight
    /// through 0.5 — which this test catches.
    #[test]
    fn v2l_discharge_floor_holds_when_stale_belief_keeps_voting_discharge() {
        use hares_equipment::Equipment as _;
        use hares_equipment::config::EquipmentConfig;
        use hares_equipment::ev::EvConfig;

        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2H {
                discharge_threshold_soc: 0.5,
                min_soc: 0.5,
            },
            0.9, // stale belief: above the discharge threshold; never reconciled
        );
        let config = EquipmentConfig::from_typed(
            "EV1".to_string(),
            "EV".to_string(),
            EvConfig {
                equipment_id: None,
                capacity_kwh: 60.0,
                charging_level: None,
                max_charging_power_kw: 7.2,
                charging_efficiency: None,
                l1_current_a: None,
                l1_voltage_v: None,
                soc_max: None,
                initial_soc: Some(0.55),
                battery_temp_c: None,
                min_charge_temp_c: None,
                full_power_temp_c: None,
                heater_power_w: None,
                heater_threshold_c: None,
                thermal_mass_j_per_k: None,
                ua_w_per_k: None,
                n_series: None,
                n_parallel: None,
                cell_resistance_ohm: None,
                v2l_enabled: Some(true),
                v2l_soc_reserve: None, // default 0.2 — strictly below the 0.5 floor
                v2l_max_discharge_kw: None,
                v2g_enabled: None,
                v2g_soc_reserve: None,
                v2g_max_discharge_kw: None,
                chemistry: None,
                fuel_economy_kwh_per_mi: None,
                ready_soc: None,
                charging_strategy: None,
                plug_in_policy: None,
                power_limit_kw: None,
                initial_connection_state: None,
                power_factor: None,
                charger_capacity_kva: None,
                cc_cv_transition_soc: None,
                charging_priority: None,
                discharge_respects_deadline: true,
            },
        )
        .expect("typed EV config");
        let mut ev = hares_equipment::ev::Ev::new(config.clone());
        ev.init(&config, &env_at_minute(19 * 60))
            .expect("EV init with valid config");

        let mut min_soc_observed = 1.0_f64;
        let mut minute = 19 * 60;
        while minute < 20 * 60 {
            let mut env = env_at_minute(minute);
            env.electrical = ElectricalSummary {
                pv_generation_kw: 0.5,
                base_load_kw: 4.0,
                ..Default::default()
            };
            let mut out = Vec::new();
            actor.decide(&env, &mut out);
            for request in &out {
                ev.apply_control_unchecked(&request.signal)
                    .expect("actor dispatch must be valid for the EV");
            }
            let mut ports = hares_types::PortSlots::default();
            ev.step(&env, std::time::Duration::from_secs(300), &mut ports)
                .expect("EV step");
            let soc = ev
                .core_output()
                .state
                .soc
                .expect("EV core output always reports SOC")
                .get();
            min_soc_observed = min_soc_observed.min(soc);
            minute += 5;
        }

        assert!(
            min_soc_observed < 0.55 - 1e-3,
            "the discharge votes must actually drain the pack for this test to mean \
             anything — a rejected dispatch would freeze SOC and prove nothing, \
             min soc observed {min_soc_observed:.4}"
        );
        assert!(
            min_soc_observed >= 0.5 - 1e-6,
            "a stale driver belief (0.9) voting discharge every step must not discharge the \
             pack below the strategy's 0.5 soc floor — the vote-carried min_soc must reach \
             compute_v2l_discharge through the resolve fold and bound the discharge; \
             instead the pack sank to {min_soc_observed:.4} (the equipment's 0.2 v2l \
              reserve fallback is the only explanation for a floor below 0.5)"
        );
    }

    // ======= Red-team: stranded-drive accounting =======

    /// A dispatched drive must deliver every kWh it dispatches — the
    /// shortfall of a drive that exceeds the pack must not vanish silently.
    /// The hold contract makes low-SOC departures reachable that the old
    /// charge-to-full default never allowed: Nightly holds outside its
    /// window, so a 07:55 plug-in at SOC 0.12 reaches the 08:00 departure
    /// with 7.2 kWh against a 9.99 kWh (30 mi) drive day. The pre-fix
    /// behavior dispatched the drive anyway and the equipment's `EvDrive`
    /// guard rejected the tail — the dwelling logged warnings the actor
    /// never read while the actor's belief subtracted the dispatched energy
    /// regardless, and the profile reported mobility the pack never
    /// delivered.
    ///
    /// The operator-directed model: a driver who cannot complete the trip
    /// does not depart and drive until the pack dies mid-route — they stay
    /// home and charge. The trip is cancelled, counted in `drive_cancelled`,
    /// and the day self-heals (the overnight window recharges the vehicle
    /// for the next departure). The finding's protective invariants hold by
    /// construction: no drive step is dispatched that the equipment would
    /// reject (none is dispatched at all), every dispatched kWh reaches the
    /// pack, and the shortfall is reported, never silently dropped.
    #[test]
    fn drive_shortfall_is_accounted_not_silently_dropped() {
        use hares_equipment::Equipment as _;

        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            0.12, // 7.2 kWh usable against the day's 9.99 kWh drive
        );
        let mut ev = make_hpxml_override_ev(0.12, None);

        // 07:55 plug-in (the urgency override tops the pack toward the band
        // for the five minutes it has), the 08:00 departure decision, then
        // the whole day and evening at 1-minute resolution through 75
        // minutes of the 22:00 window — the self-heal.
        let mut drive_signals = 0_usize;
        let mut soc_at_cancellation = 0.0_f64;
        let mut cancelled_seen = false;
        let mut anxiety_active_first_step = None;
        for step in 0..=(23 * 60 + 15 - 7 * 60 - 55) {
            let env = env_at_minute(7 * 60 + 55 + step);
            let mut out = Vec::new();
            actor.decide(&env, &mut out);
            for request in &out {
                drive_signals += matches!(request.signal, ControlSignal::EvDrive { .. }) as usize;
                ev.apply_control_unchecked(&request.signal)
                    .expect("actor dispatch must be valid for the EV");
            }
            let mut ports = hares_types::PortSlots::default();
            ev.step(&env, std::time::Duration::from_secs(60), &mut ports)
                .expect("EV step");
            if step == 0 {
                anxiety_active_first_step = actor
                    .telemetry()
                    .and_then(|t| t.get("range_anxiety_active"));
            }
            if !cancelled_seen
                && actor
                    .telemetry()
                    .and_then(|t| t.get("drive_cancelled"))
                    .is_some_and(|c| c >= 1.0)
            {
                cancelled_seen = true;
                soc_at_cancellation = ev
                    .core_output()
                    .state
                    .soc
                    .expect("EV core output always reports SOC")
                    .get();
            }
        }

        // The short driver is reported from the first plugged-in step: the
        // band state is active before any departure decision.
        assert_eq!(
            anxiety_active_first_step,
            Some(1.0),
            "a driver below the anxiety band must be reported as short on the \
             range_anxiety_active channel"
        );

        // The trip was cancelled, not driven on phantom energy: no drive
        // dispatch existed for the equipment to reject, and the shortfall
        // is counted, not silently dropped.
        assert_eq!(
            drive_signals, 0,
            "a trip the pack cannot cover must not be driven — no EvDrive may be dispatched, \
             or the profile reports mobility the pack never delivered"
        );
        assert!(
            cancelled_seen,
            "the cancelled trip must be counted in the drive_cancelled channel"
        );
        assert_eq!(
            actor.telemetry().and_then(|t| t.get("drive_cancelled")),
            Some(1.0),
            "exactly one cancellation for one uncoverable departure"
        );

        // And the day self-heals: the vehicle stayed plugged in, and by
        // 23:00 the Nightly window has recharged the pack well past the
        // cancellation-time SOC — ready for the next day's trip.
        let soc_late = ev
            .core_output()
            .state
            .soc
            .expect("EV core output always reports SOC")
            .get();
        assert!(
            soc_late > soc_at_cancellation + 0.05,
            "the cancelled day must self-heal: the overnight window must recharge the pack \
             (SOC {soc_at_cancellation:.3} at cancellation → {soc_late:.3} by 23:00)"
        );
    }

    /// A driver with away charging who arrives short of their usual amount
    /// charges out back up to it — the full top-up, not the habitual
    /// fraction — with the away session bounded at the usual target (a
    /// top-up, not a fill-to-BMS-default). Arriving home at the usual
    /// target, the evening's home cycle transfers nothing (the strategy
    /// no-ops at target — the skip is emergent and exact).
    #[test]
    fn away_charger_short_of_usual_tops_up_out_to_usual_amount() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.away_charge_fraction = 0.3;
        actor.away_charge_power_kw = 6.6;
        let mut out = Vec::new();

        // Roll event, depart, drive to completion. The day's ~9.99 kWh drive
        // leaves the driver at ≈ 0.833 — short of the usual 0.9 — so the
        // away recoup must be the full gap to usual (≈ 4.0 kWh), not the
        // habitual 0.3 × 9.99 ≈ 3.0 kWh.
        actor.decide(&env_at_minute(0), &mut out);
        out.clear();
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();
        for step in 1..=120 {
            out.clear();
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
            if matches!(actor.phase, DriverPhase::Away { .. }) {
                break;
            }
        }
        assert!(
            matches!(actor.phase, DriverPhase::Away { .. }),
            "precondition: the trip must complete into the Away phase"
        );

        // The deferred away-charge step: the session is bounded at the
        // driver's usual amount.
        out.clear();
        actor.decide(&env_at_minute(8 * 60 + 121), &mut out);
        let has_away_bound = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 1e-9
            )
        });
        assert!(
            has_away_bound,
            "the away session must be bounded at the usual amount (SOCTarget 0.9), got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
        let has_away_charge = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvAwayCharge { power_kw } if (power_kw - 6.6).abs() < 0.01
            )
        });
        assert!(
            has_away_charge,
            "the away charger must run at the configured power, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );

        // The belief credited the full top-up to the usual amount, not the
        // habitual fraction: 0.833 + 4.0/60 ≈ 0.9, versus 0.833 + 3.0/60 ≈ 0.883.
        assert!(
            (actor.perceived_soc() - 0.9).abs() < 0.01,
            "a driver short of the usual amount must be credited the full top-up to it, \
             got {}",
            actor.perceived_soc()
        );
    }

    /// The `range_anxiety_active` channel reports the driver-is-short state
    /// (perceived SOC below the anxiety band) whether or not the urgency
    /// gate has fired — predictive state for evaluators.
    #[test]
    fn range_anxiety_active_reports_short_driver() {
        // Below the band (0.2 < 0.2775), mid-day with plenty of time to the
        // next departure — the override stands down but the driver is short.
        let mut short_driver =
            make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.2);
        plugged_in_step(&mut short_driver, 12 * 60);
        assert_eq!(
            short_driver
                .telemetry()
                .and_then(|t| t.get("range_anxiety_active")),
            Some(1.0),
            "a driver below the anxiety band must read as short, even while the urgency \
             gate stands the override down"
        );

        // Above the band: not short.
        let mut comfortable_driver =
            make_plugged_in_actor(ChargingStrategy::Immediate { target_soc: 0.9 }, 0.8);
        plugged_in_step(&mut comfortable_driver, 12 * 60);
        assert_eq!(
            comfortable_driver
                .telemetry()
                .and_then(|t| t.get("range_anxiety_active")),
            Some(0.0),
            "a driver above the anxiety band must not read as short"
        );
    }

    /// A cancelled trip counts exactly once: a simulation whose step grid
    /// is not aligned to the departure minute can land a second step inside
    /// the departure-minute window (`minute_matches` spans one time
    /// resolution), and consuming the day's event on cancellation keeps the
    /// same trip from being counted twice.
    #[test]
    fn cancelled_trip_counts_once_across_departure_window() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            0.12, // 7.2 kWh against the day's 9.99 kWh trip — uncoverable
        );

        // 5-minute resolution, steps at 08:00 and 08:03 — both inside the
        // [08:00, 08:05) departure window.
        let mut env = env_at_minute(8 * 60);
        env.time_res = chrono::Duration::minutes(5);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        let mut env = env_at_minute(8 * 60 + 3);
        env.time_res = chrono::Duration::minutes(5);
        actor.decide(&env, &mut out);

        assert_eq!(
            actor.telemetry().and_then(|t| t.get("drive_cancelled")),
            Some(1.0),
            "one uncoverable trip must count as exactly one cancellation, even when a \
             second step lands inside the departure-minute window"
        );
    }
}
