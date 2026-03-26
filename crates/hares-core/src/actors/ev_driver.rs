//! EV Driver Actor — behavioral proxy for EV charging decisions.
//!
//! Models a human driver's daily routine: departure, driving (multi-step
//! energy drain), arrival, plug-in, and charging strategy selection.
//! The actor dispatches `ControlSignal` variants (`EvPlugIn`, `EvDrive`,
//! `EvSetReadyBy`, `SOCTarget`) to the EV equipment via the control pipeline.
//!
//! Equipment is self-contained with its own BMS. The driver actor only
//! pushes external decisions — it never mutates equipment state directly.

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

/// Distribution parameters for a single daily driving event pattern.
#[derive(Clone, Copy, Debug)]
pub struct EventDistributionRow {
    /// Minute-of-day the driver arrives home (0..1440).
    pub arrival_minute: u16,
    /// Duration of the away period in minutes.
    pub duration_minutes: u16,
    /// Expected SOC on arrival (0.0..=1.0), used for internal tracking.
    pub start_soc: f64,
    /// Relative probability weight for this row vs others.
    pub weight: f64,
}

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
    event_day_ratio: f64,
    arrival_fuzz_minutes: f64,
    departure_fuzz_minutes: f64,
    distributions: Vec<EventDistributionRow>,
    fuel_economy_kwh_per_mi: f64,
    capacity_kwh: f64,
    average_speed_mph: f64,
    #[allow(dead_code)] // TODO: wire into range anxiety check and away charging logic
    range_anxiety_miles: f64,
    #[allow(dead_code)] // TODO: wire into away charging fraction logic
    away_charge_fraction: f64,

    // Runtime state
    rng: ChaCha8Rng,
    current_day_ordinal: i32,
    todays_event: Option<DayEvent>,
    phase: DriverPhase,
    estimated_soc: f64,
    arrival_soc_applied: bool,
    time_res_minutes: f64,
}

/// Temperature-dependent EV driving efficiency multiplier.
///
/// Returns a multiplier on the base kWh/mile (> 1.0 = more energy consumed).
/// Based on AAA 2019 EV range testing + Geotab 2022 fleet analysis:
/// - Optimal at ~21°C (multiplier = 1.0)
/// - Cold: cabin heating + battery conditioning increase consumption
/// - Hot: AC increases consumption (less severe than cold)
///
/// Piecewise linear fit to published data:
/// - -10°C: ~1.41× (AAA: 41% range loss at 20°F)
/// - 0°C:   ~1.20× (Geotab: ~20% loss at freezing)
/// - 21°C:  1.00× (baseline)
/// - 35°C:  ~1.17× (AAA: 17% range loss at 95°F with AC)
/// - 43°C:  ~1.20× (extrapolated)
fn temp_efficiency_multiplier(ambient_c: f64) -> f64 {
    if ambient_c < 21.0 {
        // Cold: linear ramp from 1.0 at 21°C to ~1.41 at -10°C
        // Slope: 0.41 / 31 ≈ 0.0132 per °C below 21
        (1.0 + 0.0132 * (21.0 - ambient_c)).min(1.5)
    } else {
        // Hot: linear ramp from 1.0 at 21°C to ~1.17 at 35°C
        // Slope: 0.17 / 14 ≈ 0.0121 per °C above 21
        (1.0 + 0.0121 * (ambient_c - 21.0)).min(1.3)
    }
}

/// Expand a u64 seed to a [u8; 32] for ChaCha8Rng (LE bytes, zero-padded).
fn seed_bytes(seed: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&seed.to_le_bytes());
    bytes
}

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
        event_day_ratio: f64,
        arrival_fuzz_minutes: f64,
        departure_fuzz_minutes: f64,
        distributions: Vec<EventDistributionRow>,
        fuel_economy_kwh_per_mi: f64,
        capacity_kwh: f64,
        average_speed_mph: f64,
        range_anxiety_miles: f64,
        away_charge_fraction: f64,
        seed: u64,
    ) -> Self {
        Self {
            name: Arc::from(name),
            dispatch_target: DispatchTarget::ByName(target.into()),
            strategy,
            plug_in_policy,
            daily_drive_miles,
            event_day_ratio,
            arrival_fuzz_minutes,
            departure_fuzz_minutes,
            distributions,
            fuel_economy_kwh_per_mi,
            capacity_kwh,
            average_speed_mph,
            range_anxiety_miles,
            away_charge_fraction,
            rng: ChaCha8Rng::from_seed(seed_bytes(seed)),
            current_day_ordinal: -1,
            todays_event: None,
            phase: DriverPhase::HomePluggedIn,
            estimated_soc: 1.0,
            arrival_soc_applied: false,
            time_res_minutes: 0.0,
        }
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
        self.arrival_soc_applied = false;

        // Decide if today is a driving day
        let roll: f64 = self.rng.random();
        if roll >= self.event_day_ratio {
            self.todays_event = None;
            return;
        }

        // Sample distribution row
        let row = match self.sample_distribution() {
            Some(r) => r,
            None => {
                self.todays_event = None;
                return;
            }
        };

        // Sample daily miles from the ScheduleSource
        let miles = match self.daily_drive_miles.value_at(env) {
            Ok(m) => m.max(0.0),
            Err(_) => 30.0,
        };

        let temp_multiplier = temp_efficiency_multiplier(env.weather.outdoor_temp_c);
        let drive_kwh = miles * self.fuel_economy_kwh_per_mi * temp_multiplier;

        // Apply fuzz to arrival/departure
        let arrival_fuzz = if self.arrival_fuzz_minutes > 0.0 {
            let z: f64 = rand_distr::Distribution::sample(
                &rand_distr::StandardNormal,
                &mut self.rng,
            );
            (z * self.arrival_fuzz_minutes).round() as i32
        } else {
            0
        };
        let departure_fuzz = if self.departure_fuzz_minutes > 0.0 {
            let z: f64 = rand_distr::Distribution::sample(
                &rand_distr::StandardNormal,
                &mut self.rng,
            );
            (z * self.departure_fuzz_minutes).round() as i32
        } else {
            0
        };

        let arrival = (row.arrival_minute as i32 + arrival_fuzz).clamp(0, 1439) as u16;
        let departure = (row.arrival_minute as i32 - row.duration_minutes as i32 + departure_fuzz)
            .rem_euclid(1440) as u16;

        self.todays_event = Some(DayEvent {
            departure_minute: departure,
            arrival_minute: arrival,
            drive_kwh,
        });
    }

    /// Weighted random selection from distribution rows.
    fn sample_distribution(&mut self) -> Option<EventDistributionRow> {
        if self.distributions.is_empty() {
            return None;
        }
        let total_weight: f64 = self.distributions.iter().map(|r| r.weight).sum();
        if !total_weight.is_finite() || total_weight <= 0.0 {
            return None;
        }
        let draw = self.rng.random::<f64>() * total_weight;
        let mut cumulative = 0.0;
        for row in &self.distributions {
            cumulative += row.weight;
            if draw <= cumulative {
                return Some(*row);
            }
        }
        self.distributions.last().copied()
    }

    /// Check if current minute-of-day matches a target minute within time resolution.
    fn minute_matches(&self, current_minute: u16, target_minute: u16) -> bool {
        let res = self.time_res_minutes.max(1.0) as u16;
        current_minute >= target_minute && current_minute < target_minute.saturating_add(res)
    }

    /// Should the driver plug in at home based on policy?
    fn should_plug_in(&self) -> bool {
        match &self.plug_in_policy {
            PlugInPolicy::Always => true,
            PlugInPolicy::LowSoc { threshold } => self.estimated_soc < *threshold,
        }
    }

    /// Compute the next departure hour for charging deadline.
    fn next_departure_hour(&self) -> f64 {
        match &self.todays_event {
            Some(ev) => ev.departure_minute as f64 / 60.0,
            None => 7.0, // default morning departure
        }
    }

    /// Emit charging strategy signals on arrival home.
    fn emit_strategy_signals(&self, out: &mut Vec<DispatchRequest>) {
        match &self.strategy {
            ChargingStrategy::Immediate { target_soc } => {
                // Immediate: charge at full power now — emit SOCTarget, not
                // EvSetReadyBy (which would make the BMS delay).
                out.push(DispatchRequest {
                    target: self.dispatch_target.clone(),
                    signal: ControlSignal::SOCTarget {
                        target_soc: *target_soc,
                        min_soc: None,
                        max_soc: None,
                    },
                    priority: PriorityTier::Schedule,
                });
            }
            ChargingStrategy::Nightly {
                off_peak_end_hour,
                target_soc,
                ..
            } => {
                out.push(DispatchRequest {
                    target: self.dispatch_target.clone(),
                    signal: ControlSignal::EvSetReadyBy {
                        departure_hour: *off_peak_end_hour,
                        target_soc: *target_soc,
                    },
                    priority: PriorityTier::Schedule,
                });
            }
            ChargingStrategy::LowSoc { target_soc, .. } => {
                out.push(DispatchRequest {
                    target: self.dispatch_target.clone(),
                    signal: ControlSignal::SOCTarget {
                        target_soc: *target_soc,
                        min_soc: None,
                        max_soc: None,
                    },
                    priority: PriorityTier::Schedule,
                });
            }
            ChargingStrategy::QuickThenWait { partial_soc } => {
                out.push(DispatchRequest {
                    target: self.dispatch_target.clone(),
                    signal: ControlSignal::SOCTarget {
                        target_soc: *partial_soc,
                        min_soc: None,
                        max_soc: None,
                    },
                    priority: PriorityTier::Schedule,
                });
            }
            ChargingStrategy::PreDeparture { target_soc, .. } => {
                out.push(DispatchRequest {
                    target: self.dispatch_target.clone(),
                    signal: ControlSignal::EvSetReadyBy {
                        departure_hour: self.next_departure_hour(),
                        target_soc: *target_soc,
                    },
                    priority: PriorityTier::Schedule,
                });
            }
            ChargingStrategy::TouAware { target_soc, .. } => {
                out.push(DispatchRequest {
                    target: self.dispatch_target.clone(),
                    signal: ControlSignal::EvSetReadyBy {
                        departure_hour: self.next_departure_hour(),
                        target_soc: *target_soc,
                    },
                    priority: PriorityTier::Schedule,
                });
            }
            ChargingStrategy::V2H { .. }
            | ChargingStrategy::V2G { .. }
            | ChargingStrategy::SolarSurplus { .. } => {
                // V2H/V2G/SolarSurplus strategies are handled by the BMS/grid layer,
                // the driver just plugs in.
            }
        }
    }
}

impl Actor for EvDriverActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        self.time_res_minutes = env.time_res.num_seconds() as f64 / 60.0;
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
                if self.minute_matches(current_minute, event.arrival_minute) {
                    if self.should_plug_in() {
                        out.push(DispatchRequest {
                            target: self.dispatch_target.clone(),
                            signal: ControlSignal::EvPlugIn {
                                state: EvConnectionState::HomePluggedIn,
                            },
                            priority: PriorityTier::Schedule,
                        });

                        if !self.arrival_soc_applied {
                            self.emit_strategy_signals(out);
                            self.arrival_soc_applied = true;
                        }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::test_env;

    fn default_distributions() -> Vec<EventDistributionRow> {
        vec![EventDistributionRow {
            arrival_minute: 18 * 60, // 18:00
            duration_minutes: 10 * 60, // 10 hours away = depart at 08:00
            start_soc: 0.4,
            weight: 1.0,
        }]
    }

    fn make_actor(strategy: ChargingStrategy, policy: PlugInPolicy, seed: u64) -> EvDriverActor {
        EvDriverActor::new(
            "TestDriver",
            "EV1",
            strategy,
            policy,
            ScheduleSource::Constant(30.0), // 30 miles/day
            1.0,  // event every day
            0.0,  // no arrival fuzz
            0.0,  // no departure fuzz
            default_distributions(),
            0.3,  // 0.3 kWh/mi
            60.0, // 60 kWh battery
            30.0, // 30 mph average
            20.0, // 20 miles range anxiety buffer
            0.0,  // no away charging
            seed,
        )
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
    fn immediate_strategy_emits_soc_target_on_arrival() {
        let mut actor = make_actor(
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            42,
        );
        let mut out = Vec::new();

        // Step through departure
        let env_depart = env_at_minute(8 * 60);
        actor.decide(&env_depart, &mut out);

        // The first call rolls the event and checks departure.
        // With distributions: arrival=1080(18:00), duration=600 -> departure=480(08:00)
        // At minute 480, should depart
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

        let has_soc_target = out.iter().any(|r| {
            matches!(r.signal, ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.9).abs() < 0.01)
        });
        assert!(
            has_soc_target,
            "expected SOCTarget with target_soc=0.9 (Immediate charges ASAP), got: {:?}",
            out
        );
    }

    #[test]
    fn nightly_strategy_emits_set_ready_by_with_off_peak_end() {
        let mut actor = make_actor(
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.95,
            },
            PlugInPolicy::Always,
            42,
        );
        let mut out = Vec::new();

        // Depart
        actor.decide(&env_at_minute(8 * 60), &mut out);

        // Drive through
        out.clear();
        for step in 1..=100 {
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
        }

        // Arrive
        out.clear();
        actor.decide(&env_at_minute(18 * 60), &mut out);

        let ready_by = out.iter().find(|r| matches!(r.signal, ControlSignal::EvSetReadyBy { .. }));
        assert!(ready_by.is_some(), "expected EvSetReadyBy signal");

        if let Some(r) = ready_by {
            match &r.signal {
                ControlSignal::EvSetReadyBy {
                    departure_hour,
                    target_soc,
                } => {
                    assert!(
                        (*departure_hour - 6.0).abs() < 0.01,
                        "departure_hour should be off_peak_end=6.0, got {departure_hour}"
                    );
                    assert!(
                        (*target_soc - 0.95).abs() < 0.01,
                        "target_soc should be 0.95, got {target_soc}"
                    );
                }
                _ => unreachable!(),
            }
        }
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
        assert!(
            (event.drive_kwh - 9.0).abs() < 0.01,
            "drive_kwh should be 30mi * 0.3kWh/mi = 9.0, got {}",
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

        // Run both through identical sequences
        for minute in [0, 8 * 60, 8 * 60 + 30, 18 * 60] {
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
        }

        // Internal state should match
        assert_eq!(
            actor_a.todays_event.map(|e| e.drive_kwh),
            actor_b.todays_event.map(|e| e.drive_kwh),
            "daily events should match with same seed"
        );
    }

    #[test]
    fn quick_then_wait_emits_soc_target() {
        let mut actor = make_actor(
            ChargingStrategy::QuickThenWait { partial_soc: 0.5 },
            PlugInPolicy::Always,
            42,
        );
        let mut out = Vec::new();

        // Depart, drive, arrive
        actor.decide(&env_at_minute(8 * 60), &mut out);
        out.clear();
        for step in 1..=100 {
            actor.decide(&env_at_minute(8 * 60 + step), &mut out);
        }
        out.clear();
        actor.decide(&env_at_minute(18 * 60), &mut out);

        let has_soc_target = out.iter().any(|r| {
            matches!(r.signal, ControlSignal::SOCTarget { target_soc, .. } if (target_soc - 0.5).abs() < 0.01)
        });
        assert!(
            has_soc_target,
            "expected SOCTarget with partial_soc=0.5, got: {:?}",
            out
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
        // Total should be close to 30mi * 0.3kWh/mi = 9.0 kWh
        assert!(
            (total_drive_kwh - 9.0).abs() < 0.1,
            "total drive energy should be ~9.0 kWh, got {total_drive_kwh}"
        );
    }
}
