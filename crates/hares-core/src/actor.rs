//! Actor model for control decision-making.
//!
//! Actors are decision-makers (occupants, thermostats, grid operators, DR programs)
//! that push commands to equipment via control channels. Actors never directly
//! mutate environment or equipment state—they only emit [`DispatchRequest`]s
//! that flow through the [`ControlDispatcher`].
//!
//! # Key Principles
//!
//! 1. **Equipment is self-contained** with internal schedules and control logic.
//! 2. **Actors only dispatch ControlSignals** — they never directly mutate state.
//! 3. **Equipment receives signals and adjusts internal operational state**.
//! 4. **Config is separate from control signals.** Config sets up equipment at init time.

use std::sync::Arc;

use hares_control::{DispatchRequest, DispatchTarget};
use hares_types::{EnvironmentState, ZoneId};

/// What state changes an actor subscribes to. Empty = polled every step.
///
/// Dwelling can use these to skip actors whose interests haven't fired,
/// reducing decision calls ~60x for sparse actors at 1-min resolution.
#[derive(Clone, Debug, PartialEq)]
pub enum ActorInterest {
    /// Default polling — actor's `decide()` is called every timestep.
    EveryStep,
    /// Trigger when a zone temperature changes by more than `threshold_c`.
    ZoneTemperatureDelta { zone: ZoneId, threshold_c: f64 },
    /// Trigger when an equipment's mode changes.
    EquipmentModeChange { target: DispatchTarget },
    /// Trigger at a specific hour of day (0-23).
    TimeOfDay { hour: u8 },
    /// Trigger when price signal changes.
    PriceSignalChange,
}

/// Actor trait for decision-makers that emit control signals.
///
/// Implementations must be `Send + Sync` for thread-safe simulation.
/// The `decide()` method receives a pre-allocated output buffer to avoid
/// per-step allocations in the hot loop.
pub trait Actor: Send + Sync + 'static {
    /// Returns the actor's name for logging and diagnostics.
    fn name(&self) -> &str;

    /// Declares what state changes this actor cares about.
    ///
    /// Default: empty (polled every step). When the dwelling implements
    /// interest-based filtering, actors with declared interests are only
    /// called when relevant state changes occur.
    fn interests(&self) -> &[ActorInterest] {
        &[]
    }

    /// Called once per timestep (or when interests trigger).
    ///
    /// Pushes dispatch requests into the pre-allocated buffer.
    /// **MUST NOT allocate** — use the provided buffer only.
    ///
    /// # Arguments
    ///
    /// * `env` - Current environment state (weather, zone temps, grid state).
    /// * `out` - Pre-allocated output buffer to append dispatch requests to.
    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>);
}

pub mod testing {
    //! Test helpers for actor development.
    //!
    //! These utilities simplify actor testing by providing sensible defaults
    //! and assertion helpers for dispatch requests.
    //!
    //! Available for downstream actor implementations to use in their own tests.

    use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
    use hares_types::{ControlSignal, EndUse, ZoneId, ZoneState};

    use super::*;

    /// Builder for [`EnvironmentState`] with sensible defaults for actor testing.
    pub struct TestEnvBuilder {
        zone_temp_c: f64,
        outdoor_temp_c: f64,
        hour: u8,
        price_signal: hares_types::PriceSignal,
        electrical: hares_types::ElectricalSummary,
    }

    impl TestEnvBuilder {
        /// Creates a new builder with default values (21°C indoor, 10°C outdoor, hour 12).
        pub fn new() -> Self {
            Self {
                zone_temp_c: 21.0,
                outdoor_temp_c: 10.0,
                hour: 12,
                price_signal: Default::default(),
                electrical: Default::default(),
            }
        }

        /// Sets the indoor zone temperature.
        pub fn zone_temp(mut self, temp_c: f64) -> Self {
            self.zone_temp_c = temp_c;
            self
        }

        /// Sets the outdoor temperature.
        pub fn outdoor_temp(mut self, temp_c: f64) -> Self {
            self.outdoor_temp_c = temp_c;
            self
        }

        /// Sets the hour of day (0-23).
        pub fn hour(mut self, hour: u8) -> Self {
            self.hour = hour;
            self
        }

        /// Sets the price signal.
        pub fn with_price_signal(mut self, ps: hares_types::PriceSignal) -> Self {
            self.price_signal = ps;
            self
        }

        /// Sets the electrical summary.
        pub fn with_electrical(mut self, es: hares_types::ElectricalSummary) -> Self {
            self.electrical = es;
            self
        }

        /// Builds the [`EnvironmentState`].
        pub fn build(self) -> EnvironmentState {
            use chrono::{Duration, FixedOffset, TimeZone};

            EnvironmentState {
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: self.zone_temp_c,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: 14.0,
                    volume_m3: 200.0,
                }],
                weather: hares_types::WeatherState {
                    outdoor_temp_c: self.outdoor_temp_c,
                    outdoor_humidity_ratio: 0.005,
                    outdoor_wet_bulb_c: 7.0,
                    outdoor_enthalpy_j_kg: 22_800.0,
                    wind_speed_m_s: 2.0,
                    wind_dir_deg: 0.0,
                    ground_temp_c: 12.0,
                    sky_temp_c: 8.0,
                    pressure_kpa: 101.325,
                    solar_irradiance: vec![],
                    ghi_w_m2: 0.0,
                    dni_w_m2: 0.0,
                    dhi_w_m2: 0.0,
                    solar_altitude_deg: 0.0,
                    solar_azimuth_deg: 180.0,
                    mains_temp_c: 15.0,
                    rainfall_m: 0.0,
                    ground_albedo: 0.2,
                },
                grid: hares_types::GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                },
                custom_domains: vec![],
                current_time: FixedOffset::east_opt(0)
                    .expect("offset")
                    .with_ymd_and_hms(2026, 1, 1, self.hour as u32, 0, 0)
                    .single()
                    .expect("valid timestamp"),
                equipment_telemetry: std::collections::HashMap::new(),
                time_res: Duration::minutes(1),
                price_signal: self.price_signal,
                electrical: self.electrical,
            }
        }
    }

    impl Default for TestEnvBuilder {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Creates a test environment with default values.
    pub fn test_env() -> TestEnvBuilder {
        TestEnvBuilder::new()
    }

    /// Asserts that dispatch requests contain a heating thermal setpoint.
    pub fn assert_is_heating(requests: &[DispatchRequest], expected_heating_c: f64) {
        let found = requests.iter().any(|req| {
            matches!(
                &req.signal,
                ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: Some(h),
                    ..
                } if (*h - expected_heating_c).abs() < 0.01
            )
        });
        assert!(
            found,
            "no heating setpoint at {}°C found in requests: {:?}",
            expected_heating_c, requests
        );
    }

    /// Asserts that dispatch requests contain a thermal setpoint targeting a specific temperature.
    pub fn assert_target_temp(requests: &[DispatchRequest], target_c: f64) {
        let found = requests.iter().any(|req| {
            matches!(
                &req.signal,
                ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: Some(h),
                    cooling_setpoint_c: _,
                    ..
                } if (*h - target_c).abs() < 0.01
            ) || matches!(
                &req.signal,
                ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: _,
                    cooling_setpoint_c: Some(c),
                    ..
                } if (*c - target_c).abs() < 0.01
            )
        });
        assert!(
            found,
            "no thermal setpoint targeting {}°C found in requests: {:?}",
            target_c, requests
        );
    }

    /// Asserts that dispatch requests target a specific equipment by name.
    pub fn assert_target_by_name(requests: &[DispatchRequest], name: &str) {
        let found = requests
            .iter()
            .any(|req| matches!(&req.target, DispatchTarget::ByName(n) if &**n == name));
        assert!(
            found,
            "no dispatch request targeting '{}' found in requests: {:?}",
            name, requests
        );
    }

    /// Asserts that dispatch requests target a specific end-use.
    pub fn assert_target_by_end_use(requests: &[DispatchRequest], end_use: EndUse) {
        let found = requests
            .iter()
            .any(|req| matches!(&req.target, DispatchTarget::ByEndUse(e) if *e == end_use));
        assert!(
            found,
            "no dispatch request targeting {:?} found in requests: {:?}",
            end_use, requests
        );
    }

    /// Asserts the count of dispatch requests.
    pub fn assert_request_count(requests: &[DispatchRequest], expected: usize) {
        assert_eq!(
            requests.len(),
            expected,
            "expected {} dispatch requests, got {}: {:?}",
            expected,
            requests.len(),
            requests
        );
    }

    /// Creates a heating setpoint dispatch request for testing.
    pub fn heating_setpoint_request(
        target_name: &str,
        heating_c: f64,
        priority: PriorityTier,
    ) -> DispatchRequest {
        DispatchRequest {
            target: DispatchTarget::ByName(Arc::from(target_name)),
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(heating_c),
                cooling_setpoint_c: None,
                deadband_c: None,
            },
            priority,
        }
    }

    /// Creates a cooling setpoint dispatch request for testing.
    pub fn cooling_setpoint_request(
        target_name: &str,
        cooling_c: f64,
        priority: PriorityTier,
    ) -> DispatchRequest {
        DispatchRequest {
            target: DispatchTarget::ByName(Arc::from(target_name)),
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: None,
                cooling_setpoint_c: Some(cooling_c),
                deadband_c: None,
            },
            priority,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Timelike;
    use hares_control::PriorityTier;

    use super::testing::*;
    use super::*;

    struct TestActor {
        name: String,
        signals: Vec<DispatchRequest>,
    }

    impl TestActor {
        fn new(name: &str, signals: Vec<DispatchRequest>) -> Self {
            Self {
                name: name.to_string(),
                signals,
            }
        }
    }

    impl Actor for TestActor {
        fn name(&self) -> &str {
            &self.name
        }

        fn decide(&mut self, _env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
            // Move signals without cloning - TestActor is for single-use test scenarios
            out.extend(std::mem::take(&mut self.signals));
        }
    }

    #[test]
    fn actor_trait_name_returns_value() {
        let actor = TestActor::new("test_actor", vec![]);
        assert_eq!(actor.name(), "test_actor");
    }

    #[test]
    fn actor_trait_interests_default_empty() {
        let actor = TestActor::new("test_actor", vec![]);
        assert!(actor.interests().is_empty());
    }

    #[test]
    fn actor_trait_decide_emits_signals() {
        let env = test_env().build();
        let signal = heating_setpoint_request("HVAC", 20.0, PriorityTier::Schedule);
        let mut actor = TestActor::new("test_actor", vec![signal.clone()]);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0], signal);
    }

    #[test]
    fn test_env_builder_creates_valid_environment() {
        let env = test_env().zone_temp(22.0).outdoor_temp(5.0).hour(8).build();

        assert_eq!(env.zones[0].temperature_c, 22.0);
        assert_eq!(env.weather.outdoor_temp_c, 5.0);
        assert_eq!(env.current_time.hour(), 8);
    }

    #[test]
    fn assert_is_heating_finds_matching_signal() {
        let request = heating_setpoint_request("HVAC", 20.0, PriorityTier::Schedule);
        assert_is_heating(&[request], 20.0);
    }

    #[test]
    #[should_panic(expected = "no heating setpoint")]
    fn assert_is_heating_panics_on_mismatch() {
        let request = heating_setpoint_request("HVAC", 20.0, PriorityTier::Schedule);
        assert_is_heating(&[request], 25.0);
    }

    #[test]
    fn assert_target_by_name_finds_match() {
        let request = heating_setpoint_request("HVAC", 20.0, PriorityTier::Schedule);
        assert_target_by_name(&[request], "HVAC");
    }

    #[test]
    fn assert_request_count_validates_count() {
        let requests = vec![
            heating_setpoint_request("HVAC1", 20.0, PriorityTier::Schedule),
            cooling_setpoint_request("HVAC2", 24.0, PriorityTier::UserOverride),
        ];
        assert_request_count(&requests, 2);
    }
}
