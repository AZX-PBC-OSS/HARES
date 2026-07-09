//! EV Driver Actor -- behavioral proxy for EV charging decisions.
//!
//! Models a human driver's daily routine: departure, driving (multi-step
//! energy drain), arrival, plug-in, and charging strategy selection.
//! The actor dispatches `ControlSignal` variants (`EvPlugIn`, `EvDrive`,
//! `EvSetReadyBy`, `SOCTarget`) to the EV equipment via the control pipeline.
//!
//! Equipment is self-contained with its own BMS. The driver actor only
//! pushes external decisions -- it never mutates equipment state directly.
//!
//! ## SOC estimation model
//!
//! The actor maintains `estimated_soc` as its best guess of battery state.
//! This intentionally diverges from actual equipment SOC because the actor
//! does not observe CC-CV taper, thermal derating, or BMS charge termination.
//! All driver behavioral decisions -- plug-in, range anxiety, charging strategy
//! -- operate on `perceived_soc()` which returns `estimated_soc`. The ground-truth
//! `actual_soc()` reads `equipment_core` and exists only for reconciliation,
//! observability, and diagnostics. The divergence is bounded and conservative:
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
use self::preference::{ChargingPreference, DecisionContext};
use self::price::PriceOptimizer;
use self::soc_gate::SocGate;
use self::soc_target::SocTarget;
use self::solar::SolarTracking;
use self::time_window::TimeWindowPref;
use self::v2g::V2GExport;
use self::v2h::V2HDischarge;

/// A rolled daily driving event.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
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
    /// Currently driving (multi-step drain).
    Driving {
        remaining_kwh: f64,
        total_steps: u32,
        steps_done: u32,
    },
    /// Away from home (parked, not driving).
    Away,
}

/// Hysteresis band (SOC fraction) for SocGate lower_threshold.
///
/// 0.05 (5 percentage points) is sufficient to suppress rapid charge/no-charge
/// cycling from typical BMS estimation noise, self-discharge rates, and
/// auxiliary-load fluctuations in residential EV use. The band is applied
/// symmetrically below the upper threshold for both LowSoc and QuickThenWait
/// strategies.
const SOC_GATE_DEFAULT_HYSTERESIS_BAND: f64 = 0.05;

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
                charging_allowed: true,
            })]
        }
        ChargingStrategy::QuickThenWait { partial_soc } => {
            vec![Box::new(SocGate {
                upper_threshold: *partial_soc,
                lower_threshold: *partial_soc - SOC_GATE_DEFAULT_HYSTERESIS_BAND,
                target_soc: *partial_soc,
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
                Box::new(SocTarget { target_soc: 0.9 }),
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
                Box::new(SocTarget { target_soc: 1.0 }),
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
        let prefs = build_preferences(
            &strategy,
            max_charge_kw,
            0.9, // charging efficiency for energy calc
            None,
            24,
        );
        let composer = ChargingComposer::new(prefs, target);
        let expected_daily_miles = daily_drive_miles.mean();

        #[cfg(feature = "observe")]
        tracing::debug!(
            actor = name,
            distribution = ?daily_drive_miles,
            analytical_mean_mi = expected_daily_miles,
            "EV driver daily-miles distribution"
        );

        let mut telemetry = Telemetry::with_capacity(7);
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
            telemetry,
        }
    }

    /// Set the price schedule for TOU-aware strategies, rebuilding the composer.
    pub fn with_price_schedule(mut self, schedule: Arc<[f64]>, steps_per_day: usize) -> Self {
        let target = self.target_name().to_owned();
        let prefs = build_preferences(
            &self.strategy,
            self.max_charge_kw,
            0.9,
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

    fn populate_telemetry(&mut self, before_out: usize, out: &[DispatchRequest]) {
        self.telemetry.set("soc", self.estimated_soc);
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
        let mut charge_kw = 0.0;
        let mut discharge_min_soc = 0.0;
        for req in &out[before_out..] {
            match &req.signal {
                ControlSignal::PowerSetpoint {
                    active_power_kw,
                    min_soc,
                    ..
                } => {
                    charge_kw = *active_power_kw;
                    // Why: None min_soc on a PowerSetpoint means "no discharge
                    // floor constraint" — only V2G/V2H strategies set min_soc.
                    // 0.0 = "allow full discharge" is the default operational
                    // behaviour, so the sentinel is semantically correct.
                    discharge_min_soc = min_soc.unwrap_or(0.0);
                }
                ControlSignal::EvAwayCharge { power_kw } => charge_kw = *power_kw,
                ControlSignal::EvDrive { .. } => {} // driving, not charging
                _ => {}
            }
        }
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
    }

    /// Returns the target equipment name.
    pub fn target_name(&self) -> &str {
        match &self.dispatch_target {
            DispatchTarget::ByName(n) => n,
            DispatchTarget::ByEndUse(_) => unreachable!("EvDriverActor always targets by name"),
        }
    }

    pub fn resolve_equipment_id(&mut self, equipment_id_by_name: &HashMap<String, EquipmentId>) {
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

    /// Roll a daily event for a new day if needed.
    fn maybe_roll_daily_event(&mut self, env: &EnvironmentState) {
        let ordinal = env.current_time.ordinal0() as i32 + env.current_time.year() * 366;
        if ordinal == self.current_day_ordinal {
            return;
        }
        self.current_day_ordinal = ordinal;

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

    /// Should the driver plug in at home based on perceived SOC and policy.
    fn should_plug_in(&self) -> bool {
        match &self.plug_in_policy {
            PlugInPolicy::Always => true,
            PlugInPolicy::LowSoc { threshold } => self.perceived_soc() < *threshold,
        }
    }

    /// Check if tomorrow's expected trip would leave perceived SOC dangerously low.
    /// If so, the driver overrides their strategy and charges to full.
    fn needs_range_anxiety_override(&self, env: &EnvironmentState) -> bool {
        if self.range_anxiety_miles <= 0.0 {
            return false;
        }
        let ambient_c = env.weather.outdoor_temp_c;
        let soc = self.perceived_soc();
        let temp_mult = temp_efficiency_multiplier(ambient_c);
        let anxiety_kwh = (self.expected_daily_miles + self.range_anxiety_miles)
            * self.fuel_economy_kwh_per_mi
            * temp_mult;
        let anxiety_soc = anxiety_kwh / self.capacity_kwh.max(0.01);
        soc < anxiety_soc
    }

    /// Evaluate the composer per-step while plugged in at home.
    fn evaluate_charging(
        &mut self,
        env: &EnvironmentState,
        current_minute: u16,
        out: &mut Vec<DispatchRequest>,
    ) {
        // Range anxiety override: if tomorrow's trip would strand the driver,
        // charge to full regardless of strategy.
        // `Schedule` tier — the EV driver is a schedule-level actor; this
        // override is a pre-defined operational rule, not a user or grid action.
        if self.needs_range_anxiety_override(env) {
            out.push(DispatchRequest {
                target: self.dispatch_target.clone(),
                signal: ControlSignal::SOCTarget {
                    target_soc: 1.0,
                    min_soc: None,
                    max_soc: None,
                },
                priority: PriorityTier::Schedule,
            });
            return;
        }

        let soc = self.perceived_soc();
        let ctx = DecisionContext {
            current_soc: soc,
            capacity_kwh: self.capacity_kwh,
            max_charge_kw: self.max_charge_kw,
            max_discharge_kw: self.max_charge_kw,
            env,
            current_minute,
            next_departure_minute: self.todays_event.map(|e| e.departure_minute),
            time_res_minutes: self.time_res_minutes,
        };
        self.composer.evaluate(&ctx, out);

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
}

impl Actor for EvDriverActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
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

        let event = match self.todays_event {
            Some(ev) => ev,
            None => {
                self.populate_telemetry(before_out, out);
                return;
            }
        };

        let current_minute = (env.current_time.hour() * 60 + env.current_time.minute()) as u16;

        match self.phase {
            DriverPhase::HomePluggedIn => {
                if self.minute_matches(current_minute, event.departure_minute) {
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
            } => {
                let steps_left = total_steps.saturating_sub(steps_done);
                if steps_left == 0 {
                    self.phase = DriverPhase::Away;
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
                        let recoup_kwh = event.drive_kwh * self.away_charge_fraction;
                        let recoup_soc = recoup_kwh / self.capacity_kwh.max(0.01);
                        self.estimated_soc = (self.estimated_soc + recoup_soc).min(1.0);
                        self.needs_away_charge = true;
                    }
                    self.phase = DriverPhase::Away;
                } else {
                    self.phase = DriverPhase::Driving {
                        remaining_kwh: new_remaining,
                        total_steps,
                        steps_done: new_steps_done,
                    };
                }
            }
            DriverPhase::Away => {
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
                    out.push(DispatchRequest {
                        target: self.dispatch_target.clone(),
                        signal: ControlSignal::EvAwayCharge {
                            power_kw: self.away_charge_power_kw,
                        },
                        priority: PriorityTier::Schedule,
                    });
                }

                if self.minute_matches(current_minute, event.arrival_minute) {
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

                    tracing::debug!(
                        actor = %self.name,
                        target = self.target_name(),
                        arrival_minute = event.arrival_minute,
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

        self.populate_telemetry(before_out, out);
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
        Ok(())
    }

    fn rng_pair(&self) -> Option<([u8; 32], u64)> {
        Some((self.rng.get_seed(), self.rng.get_stream()))
    }
}

fn phase_as_f64(phase: DriverPhase) -> f64 {
    match phase {
        DriverPhase::HomePluggedIn => 0.0,
        DriverPhase::Driving { .. } => 1.0,
        DriverPhase::Away => 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::test_env;
    use hares_types::{CoreOutput, CoreState, ElectricalSummary, EquipmentId, PriceSignal, Soc};

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

    fn env_at_minute(minute: u16) -> EnvironmentState {
        let hour = (minute / 60).min(23) as u8;
        let min = minute % 60;
        use chrono::{FixedOffset, TimeZone};
        let tz = FixedOffset::east_opt(0).unwrap();
        let mut env = test_env().hour(hour).build();
        env.current_time = tz
            .with_ymd_and_hms(2026, 1, 1, hour as u32, min as u32, 0)
            .single()
            .unwrap();
        env
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

        let has_charging_signal = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { .. }
                    | ControlSignal::EvSetReadyBy { .. }
                    | ControlSignal::PowerSetpoint { .. }
            )
        });
        assert!(
            !has_charging_signal,
            "nightly should idle outside off-peak window at 18:01, got: {:?}",
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
        // SocGate overrides idle above threshold.
        let has_charging = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { .. }
                    | ControlSignal::PowerSetpoint { .. }
                    | ControlSignal::EvSetReadyBy { .. }
            )
        });
        assert!(
            !has_charging,
            "QuickThenWait should idle when SOC above partial_soc=0.5, got: {:?}",
            out.iter().map(|r| &r.signal).collect::<Vec<_>>()
        );
    }

    #[test]
    fn no_event_day_emits_nothing() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        actor.event_day_ratio = 0.0; // never a driving day
        let mut out = Vec::new();

        actor.decide(&env_at_minute(8 * 60), &mut out);
        assert!(out.is_empty(), "no signals on non-driving day");

        actor.decide(&env_at_minute(18 * 60), &mut out);
        assert!(out.is_empty(), "no signals on non-driving day");
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
    fn range_anxiety_overrides_strategy_when_soc_low() {
        let mut actor = make_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            PlugInPolicy::Always,
            42,
        );
        actor.estimated_soc = 0.15;
        let mut out = Vec::new();

        // Roll event, depart, drive, arrive
        actor.decide(&env_at_minute(0), &mut out);
        out.clear();
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();
        for step in 1..=120 {
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
            out.clear();
        }
        actor.decide(&env_at_minute(18 * 60), &mut out);
        out.clear();

        // Post-arrival step: range anxiety should override
        actor.decide(&env_at_minute(18 * 60 + 1), &mut out);

        let has_soc_target_full = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 1.0).abs() < 0.01
            )
        });
        assert!(
            has_soc_target_full,
            "range anxiety should override to SOCTarget(1.0), got: {:?}",
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
            if matches!(actor.phase, DriverPhase::Away) && actor.needs_away_charge {
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

        // Force Away phase with no pending away-charge signals
        actor.phase = DriverPhase::Away;
        actor.needs_away_charge = false;

        // Pick a minute that is NOT near arrival (1080). 15:00 = 900.
        actor.decide(&env_at_minute(15 * 60), &mut out);
        assert!(
            out.is_empty(),
            "Away phase should emit nothing when not at arrival minute, got: {out:?}"
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

        let has_charging = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { .. }
                    | ControlSignal::PowerSetpoint { .. }
                    | ControlSignal::EvSetReadyBy { .. }
            )
        });
        assert!(
            !has_charging,
            "Nightly should idle during peak hours, got: {:?}",
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

        let has_charging = out.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::SOCTarget { .. }
                    | ControlSignal::PowerSetpoint { .. }
                    | ControlSignal::EvSetReadyBy { .. }
            )
        });
        assert!(
            !has_charging,
            "LowSoc should idle above threshold, got: {out:?}"
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
            if matches!(actor.phase, DriverPhase::Away) {
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
    // On a non-driving day todays_event is None, so decide() exits before reaching
    // evaluate_charging(). We test needs_range_anxiety_override() directly (accessible
    // from within the same module's test block) to confirm the predicate is true when
    // SOC is below the anxiety threshold regardless of whether a trip is scheduled.
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
        // expensive price → negative discharge PowerSetpoint (no positive charge).
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
            ) || matches!(r.signal, ControlSignal::SOCTarget { .. })
        });
        assert!(
            !has_positive_charge,
            "TOU should not charge at expensive price (should discharge instead), got: {out_expensive:?}"
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

        // Evening 18:00-21:59: outside off-peak window, should idle
        for hour in 18..22 {
            let out = plugged_in_step(&mut actor, hour * 60);
            let has_charging = out.iter().any(|r| {
                matches!(
                    r.signal,
                    ControlSignal::SOCTarget { .. }
                        | ControlSignal::PowerSetpoint { .. }
                        | ControlSignal::EvSetReadyBy { .. }
                )
            });
            assert!(
                !has_charging,
                "nightly should idle at hour {hour} (before off-peak), got: {:?}",
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

        // Morning 06:00-07:59: outside off-peak again, should idle
        for hour in 6..8 {
            let out = plugged_in_step(&mut actor, hour * 60);
            let has_charging = out.iter().any(|r| {
                matches!(
                    r.signal,
                    ControlSignal::SOCTarget { .. }
                        | ControlSignal::PowerSetpoint { .. }
                        | ControlSignal::EvSetReadyBy { .. }
                )
            });
            assert!(
                !has_charging,
                "nightly should idle at hour {hour} (after off-peak), got: {:?}",
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

        // High SOC: all scores are 0, so the resolved label is soc_target with
        // target_soc=0.9 and score=0. Equipment handles the no-op.
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
    fn multi_step_driving_soc_divergence_accumulates() {
        // Simulate multiple complete drive cycles. After each trip the
        // actor deducts energy from estimated_soc but equipment_core
        // remains unchanged, so the estimates diverge.
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let mut env = env_at_minute(0);
        set_core_soc(&mut actor, &mut env, "EV1", EquipmentId(7), 1.0);
        // Run 3 complete drive cycles (depart→drive→arrive→home)
        for _cycle in 0..3 {
            let mut out = Vec::new();
            // Depart at 08:00, drive through to 18:00
            for minute in 0_u16..1440 {
                env.current_time = env_at_minute(minute).current_time;
                out.clear();
                actor.decide(&env, &mut out);
            }
        }
        let perceived = actor.perceived_soc();
        let actual = actor.actual_soc(&env);
        assert!(
            perceived < actual.unwrap() - 0.1,
            "after 3 trips without reconciliation, estimated_soc should be below actual equipment SOC"
        );
        assert_eq!(
            actual,
            Some(1.0),
            "equipment_core SOC should remain unchanged at 1.0"
        );
    }
}
