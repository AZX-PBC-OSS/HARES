//! EV Driver Actor — behavioral proxy for EV charging decisions.
//!
//! Models a human driver's daily routine: departure, driving (multi-step
//! energy drain), arrival, plug-in, and charging strategy selection.
//! The actor dispatches `ControlSignal` variants (`EvPlugIn`, `EvDrive`,
//! `EvSetReadyBy`, `SOCTarget`) to the EV equipment via the control pipeline.
//!
//! Equipment is self-contained with its own BMS. The driver actor only
//! pushes external decisions — it never mutates equipment state directly.

mod composer;
mod departure;
mod efficiency;
mod preference;
mod price;
mod soc_gate;
mod soc_target;
mod solar;
mod time_window;
mod v2h;
mod v2g;

use std::sync::Arc;

use chrono::{Datelike, Timelike};
use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::{
    ChargingStrategy, ControlSignal, EnvironmentState, EvConnectionState, PlugInPolicy,
    ScheduleSource,
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
#[derive(Clone, Copy, Debug)]
struct DayEvent {
    departure_minute: u16,
    arrival_minute: u16,
    drive_kwh: f64,
}

/// State machine phase for the driver's day.
#[derive(Clone, Copy, Debug, PartialEq)]
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
                threshold: *threshold,
                target_soc: *target_soc,
            })]
        }
        ChargingStrategy::QuickThenWait { partial_soc } => {
            vec![Box::new(SocGate {
                threshold: *partial_soc,
                target_soc: *partial_soc,
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

    // Behavioral config
    strategy: ChargingStrategy,
    plug_in_policy: PlugInPolicy,
    daily_drive_miles: ScheduleSource,
    departure_time: ScheduleSource,
    trip_duration: ScheduleSource,
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
}

use efficiency::{seed_bytes, temp_efficiency_multiplier};

impl EvDriverActor {
    /// Creates a new EV driver actor.
    ///
    /// `seed` is required for deterministic behavior. All stochastic draws
    /// derive from this single seed.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        target: &str,
        strategy: ChargingStrategy,
        plug_in_policy: PlugInPolicy,
        daily_drive_miles: ScheduleSource,
        departure_time: ScheduleSource,
        trip_duration: ScheduleSource,
        event_day_ratio: f64,
        fuel_economy_kwh_per_mi: f64,
        capacity_kwh: f64,
        max_charge_kw: f64,
        average_speed_mph: f64,
        range_anxiety_miles: f64,
        away_charge_fraction: f64,
        away_charge_power_kw: f64,
        seed: u64,
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
        Self {
            name: Arc::from(name),
            dispatch_target: DispatchTarget::ByName(target.into()),
            strategy,
            plug_in_policy,
            daily_drive_miles,
            departure_time,
            trip_duration,
            event_day_ratio,
            fuel_economy_kwh_per_mi,
            capacity_kwh,
            max_charge_kw,
            average_speed_mph,
            range_anxiety_miles,
            away_charge_fraction,
            away_charge_power_kw,
            composer,
            rng: ChaCha8Rng::from_seed(seed_bytes(seed)),
            current_day_ordinal: -1,
            todays_event: None,
            phase: DriverPhase::HomePluggedIn,
            estimated_soc: 1.0,
            time_res_minutes: 0.0,
            expected_daily_miles,
            needs_away_charge: false,
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

    /// Returns the target equipment name.
    pub fn target_name(&self) -> &str {
        match &self.dispatch_target {
            DispatchTarget::ByName(n) => n,
            DispatchTarget::ByEndUse(_) => unreachable!("EvDriverActor always targets by name"),
        }
    }

    /// Roll a daily event for a new day if needed.
    fn maybe_roll_daily_event(&mut self, env: &EnvironmentState) {
        let ordinal = env.current_time.ordinal0() as i32
            + env.current_time.year() * 366;
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

        // Sample departure time and duration from ScheduleSource (seeded, reproducible)
        let departure_min = self
            .departure_time
            .value_at(env)
            .unwrap_or(480.0)
            .clamp(0.0, 1439.0) as u16;

        let duration_min = self
            .trip_duration
            .value_at(env)
            .unwrap_or(600.0)
            .clamp(30.0, 1200.0) as u16;

        let arrival = (departure_min as u32 + duration_min as u32).min(1439) as u16;

        // Sample daily miles from the ScheduleSource
        let miles = self
            .daily_drive_miles
            .value_at(env)
            .unwrap_or(30.0)
            .max(0.0);

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

    /// Read the actual SOC from equipment telemetry, falling back to estimated.
    fn current_soc(&self, env: &EnvironmentState) -> f64 {
        let target_name = match &self.dispatch_target {
            DispatchTarget::ByName(name) => name.as_ref(),
            _ => return self.estimated_soc,
        };
        env.equipment_telemetry
            .get(target_name)
            .and_then(|tel| tel.get("soc"))
            .unwrap_or(self.estimated_soc)
    }

    /// Should the driver plug in at home based on policy?
    fn should_plug_in(&self, env: &EnvironmentState) -> bool {
        match &self.plug_in_policy {
            PlugInPolicy::Always => true,
            PlugInPolicy::LowSoc { threshold } => self.current_soc(env) < *threshold,
        }
    }

    /// Check if tomorrow's expected trip would leave SOC dangerously low.
    /// If so, the driver overrides their strategy and charges to full.
    fn needs_range_anxiety_override(&self, env: &EnvironmentState) -> bool {
        if self.range_anxiety_miles <= 0.0 {
            return false;
        }
        let ambient_c = env.weather.outdoor_temp_c;
        let soc = self.current_soc(env);
        let temp_mult = temp_efficiency_multiplier(ambient_c);
        let anxiety_kwh = (self.expected_daily_miles + self.range_anxiety_miles)
            * self.fuel_economy_kwh_per_mi
            * temp_mult;
        let anxiety_soc = anxiety_kwh / self.capacity_kwh.max(0.01);
        soc < anxiety_soc
    }

    /// Evaluate the composer per-step while plugged in at home.
    fn evaluate_charging(&mut self, env: &EnvironmentState, current_minute: u16, out: &mut Vec<DispatchRequest>) {
        // Range anxiety override: if tomorrow's trip would strand the driver,
        // charge to full regardless of strategy.
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

        let soc = self.current_soc(env);
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

impl Actor for EvDriverActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        let res_seconds = env.time_res.num_seconds();
        debug_assert!(res_seconds >= 1, "time_res must be >= 1 second");
        self.time_res_minutes = (res_seconds.max(1) as f64) / 60.0;
        self.maybe_roll_daily_event(env);

        let event = match self.todays_event {
            Some(ev) => ev,
            None => return,
        };

        let current_minute =
            (env.current_time.hour() * 60 + env.current_time.minute()) as u16;

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
                    let total_steps =
                        (trip_minutes / self.time_res_minutes).ceil().max(1.0) as u32;

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
                    signal: ControlSignal::EvDrive {
                        kwh: kwh_this_step,
                    },
                    priority: PriorityTier::Schedule,
                });

                self.estimated_soc =
                    (self.estimated_soc - kwh_this_step / self.capacity_kwh.max(0.01)).max(0.0);

                let new_steps_done = steps_done + 1;
                let new_remaining = remaining_kwh - kwh_this_step;

                if new_steps_done >= total_steps {
                    // Trip complete — defer away-charge signals to the next
                    // step so EvDrive is fully processed before EvPlugIn.
                    if self.away_charge_fraction > 0.0 {
                        let recoup_kwh =
                            event.drive_kwh * self.away_charge_fraction;
                        let recoup_soc = recoup_kwh / self.capacity_kwh.max(0.01);
                        self.estimated_soc =
                            (self.estimated_soc + recoup_soc).min(1.0);
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

                    if self.should_plug_in(env) {
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
                        plugged_in = self.should_plug_in(env),
                        "EV driver arrived home",
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::test_env;
    use hares_types::{ElectricalSummary, PriceSignal};

    fn make_actor(strategy: ChargingStrategy, policy: PlugInPolicy, seed: u64) -> EvDriverActor {
        EvDriverActor::new(
            "TestDriver",
            "EV1",
            strategy,
            policy,
            ScheduleSource::Constant(30.0),  // 30 miles/day
            ScheduleSource::Constant(480.0),  // depart 08:00
            ScheduleSource::Constant(600.0),  // 10h away → arrive 18:00
            1.0,  // event every day
            0.3,  // 0.3 kWh/mi
            60.0, // 60 kWh battery
            7.2,  // L2 charge rate
            30.0, // 30 mph average
            20.0, // 20 miles range anxiety buffer
            0.0,  // no away charging
            6.6,  // workplace L2 default
            seed,
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

        // Arrive at 18:00 — plug in, transition to HomePluggedIn
        actor.decide(&env_at_minute(18 * 60), &mut out);
        out.clear();

        // Next step at 18:01 — now in HomePluggedIn, composer evaluates
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

        // Arrive at 18:00, check at 18:01 — outside off-peak window (22:00-06:00)
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

        // Skip to arrival — SOC will be high since 30mi * 0.3kWh/mi = 9kWh out of 60kWh
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

        // One more step in Away phase — deferred signals are emitted here.
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
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            0.4,
        );
        let out = plugged_in_step(&mut actor, 19 * 60);

        let has_soc_target = out.iter().any(|r| {
            matches!(r.signal, ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 0.01)
        });
        assert!(has_soc_target, "Immediate should charge to 0.9, got: {out:?}");
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
            ChargingStrategy::LowSoc { threshold: 0.5, target_soc: 0.8 },
            0.3,
        );
        let out = plugged_in_step(&mut actor, 19 * 60);

        let has_soc_target = out.iter().any(|r| {
            matches!(r.signal, ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.8).abs() < 0.01)
        });
        assert!(has_soc_target, "LowSoc should charge below threshold, got: {out:?}");
    }

    #[test]
    fn ev_low_soc_above_threshold_idles() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::LowSoc { threshold: 0.5, target_soc: 0.8 },
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
        assert!(!has_charging, "LowSoc should idle above threshold, got: {out:?}");
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
        assert!(has_signal, "TOU with uniform prices should still charge, got: {out:?}");
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
        assert!(has_power, "SolarSurplus should modulate to surplus (3.5kW), got: {out:?}");
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
        assert!(!has_power, "SolarSurplus should idle below min rate, got: {out:?}");
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
        assert!(has_signal, "PreDeparture should plan charging, got: {out:?}");
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
        assert!(has_urgent, "PreDeparture should urgently charge near deadline, got: {out:?}");
    }

    #[test]
    fn ev_v2h_discharges_during_deficit() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2H { discharge_threshold_soc: 0.5, min_soc: 0.2 },
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
            if let ControlSignal::PowerSetpoint { active_power_kw, .. } = &r.signal {
                if *active_power_kw < 0.0 { Some(*active_power_kw) } else { None }
            } else {
                None
            }
        });
        assert!(discharge_power.is_some(), "V2H should discharge during deficit, got: {out:?}");
        let power = discharge_power.unwrap();
        assert!(
            (power + 3.0).abs() < 0.1,
            "V2H deficit = 4.0-1.0 = 3.0 kW, expected power ~ -3.0, got {power}"
        );
    }

    #[test]
    fn ev_v2h_idles_with_low_soc() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2H { discharge_threshold_soc: 0.5, min_soc: 0.2 },
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
        assert!(!has_discharge, "V2H should not discharge below min_soc, got: {out:?}");
    }

    #[test]
    fn ev_v2g_discharges_above_price() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2G { min_soc: 0.3, max_export_kw: 5.0, price_threshold: 0.20 },
            0.7,
        );
        let mut env = env_at_minute(19 * 60);
        env.price_signal = PriceSignal {
            electricity_price: Some(0.30),
            ..Default::default()
        };
        let out = plugged_in_step_with_env(&mut actor, &env);

        let discharge_power = out.iter().find_map(|r| {
            if let ControlSignal::PowerSetpoint { active_power_kw, .. } = &r.signal {
                if *active_power_kw < 0.0 { Some(*active_power_kw) } else { None }
            } else {
                None
            }
        });
        assert!(discharge_power.is_some(), "V2G should discharge above price threshold, got: {out:?}");
        let power = discharge_power.unwrap();
        assert!(
            (power + 5.0).abs() < 0.1,
            "V2G max_export_kw=5.0, expected power ~ -5.0, got {power}"
        );
    }

    #[test]
    fn ev_v2g_idles_below_price() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::V2G { min_soc: 0.3, max_export_kw: 5.0, price_threshold: 0.20 },
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
        assert!(!has_discharge, "V2G should not discharge below price threshold, got: {out:?}");
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
            1.0,
            0.3,
            60.0,
            7.2,
            30.0,
            20.0,
            0.5,  // 50% away charge
            6.6,
            seed,
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
        assert!(has_away_now, "first Away step should emit deferred AwayPluggedIn");
        assert!(has_away_charge, "first Away step should emit deferred EvAwayCharge");

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
        assert!(!has_away_again, "subsequent Away steps should not re-emit AwayPluggedIn");
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

        // At 06:00, only 1 hour until departure. Need 0.6 * 60kWh / (7.2 * 0.9) = 5.6h.
        // 1h << 5.6h * 1.2 = urgent override fires.
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
            ) || matches!(
                r.signal,
                ControlSignal::EvSetReadyBy { .. }
            ) || matches!(
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
        // SOC 0.3, need 0.7 * 60 / (7.2 * 0.9) = 6.48h, 8h > 6.48 * 1.2 = not urgent.
        // Solar has nothing -> should idle.
        let mut env1 = env_at_minute(23 * 60);
        env1.electrical = ElectricalSummary {
            pv_generation_kw: 0.0,
            base_load_kw: 1.0,
            ..Default::default()
        };
        let out1 = plugged_in_step_with_env(&mut actor, &env1);
        // No PowerSetpoint should fire — solar has no surplus.
        // EvSetReadyBy may still be emitted (departure planning), which is fine —
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
        // Need 6.48h but only 0.5h -> departure override fires.
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
        assert!(has_charge, "TOU should charge at cheap price, got: {out_cheap:?}");

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
            ) || matches!(
                r.signal,
                ControlSignal::SOCTarget { .. }
            )
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
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            0.3,
        );

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
            if let ControlSignal::PowerSetpoint { active_power_kw, .. } = &r.signal {
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
            if let ControlSignal::PowerSetpoint { active_power_kw, .. } = &r.signal {
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
                ControlSignal::SOCTarget { .. }
                    | ControlSignal::PowerSetpoint { .. }
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
        assert_eq!(prefs.len(), 3, "TouAware should install 3 preferences (PriceOptimizer + DepartureDeadline + SocTarget)");
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
        assert_eq!(prefs.len(), 1, "Immediate should install 1 preference (SocTarget)");
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
        assert_eq!(prefs.len(), 2, "SolarSurplus should install 2 preferences (SolarTracking + DepartureDeadline)");
    }

    // Finding 6: evaluate_charging path not proven
    #[test]
    fn evaluate_charging_updates_last_action() {
        let mut actor = make_plugged_in_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            0.5,
        );

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
            ) || matches!(
                r.signal,
                ControlSignal::EvSetReadyBy { .. }
            ) || matches!(
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
}
