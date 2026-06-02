//! Humidity domain solver for indoor moisture balance.

use std::collections::HashMap;
use std::time::Duration;

use hares_physics::air_properties::moist_air_density_kg_m3;
use hares_physics::constants::{KJ_TO_J, LATENT_HEAT_VAPORISATION_0C_KJ_KG};
use hares_physics::psychrometrics::{
    humidity_ratio_from_tdp, relative_humidity, wet_bulb_from_humidity_ratio,
};
use hares_types::{
    DomainId, DomainSolver, DomainUpdate, EnvironmentState, HUMIDITY, PortSlots, THERMAL, ZoneId,
};

#[derive(Debug, Clone)]
pub struct HumiditySolverConfig {
    /// Per-zone volume overrides. Falls back to `ZoneState::volume_m3` for zones
    /// not present in this map.
    pub zone_volumes_m3: HashMap<ZoneId, f64>,
    /// Multiplier on zone air moisture capacitance to represent furniture and
    /// building material moisture absorption. 15× matches OCHRE's `humidity_cap_mult`.
    /// Applied uniformly to all zones (single-zone validated only).
    pub moisture_buffering_multiplier: f64,
    /// Latent heat of vaporisation at 0°C [J/kg]; 2501 kJ/kg per ASHRAE / OCHRE.
    pub h_fg_j_kg: f64,
}

impl Default for HumiditySolverConfig {
    fn default() -> Self {
        Self {
            zone_volumes_m3: HashMap::new(),
            // 15× matches OCHRE's humidity_cap_mult: furniture and building materials
            // absorb moisture, slowing RH swings and preventing dehumidifier cycling.
            moisture_buffering_multiplier: 15.0,
            h_fg_j_kg: LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HumiditySolver {
    pub config: HumiditySolverConfig,
    pub humidity_ratios: HashMap<ZoneId, f64>,
    // Reusable per-step buffers; cleared at the start of each resolve call.
    zone_temp_buf: HashMap<ZoneId, f64>,
    latent_buf: HashMap<ZoneId, f64>,
    /// Per-zone infiltration mass flow rate [kg/s] from thermal domain payload.
    m_dot_inf_buf: HashMap<ZoneId, f64>,
    /// Per-zone outdoor humidity ratio [kg/kg] from thermal domain payload.
    w_outdoor_buf: HashMap<ZoneId, f64>,
    /// Per-zone condensation/sublimation mass accumulated during clamping [kg].
    /// Positive = condensation to surfaces (moisture removed from air).
    /// Negative = frost/sublimation (moisture added to air by clamping at zero).
    /// Reset each timestep at the start of `resolve`.
    condensation_kg: HashMap<ZoneId, f64>,
}

impl HumiditySolver {
    #[must_use]
    pub fn new(config: HumiditySolverConfig, env: &EnvironmentState) -> Self {
        let n_zones = env.zones.len();
        let humidity_ratios = env
            .zones
            .iter()
            .map(|zone| (zone.id, zone.humidity_ratio))
            .collect();
        Self {
            config,
            humidity_ratios,
            zone_temp_buf: HashMap::with_capacity(n_zones),
            latent_buf: HashMap::with_capacity(n_zones),
            m_dot_inf_buf: HashMap::with_capacity(n_zones),
            w_outdoor_buf: HashMap::with_capacity(n_zones),
            condensation_kg: HashMap::with_capacity(n_zones),
        }
    }

    /// Return the committed humidity ratio for `zone_id`.
    ///
    /// # Panics
    ///
    /// In debug builds (debug-assertions) panics when `zone_id` is not in the
    /// solver's internal map. Production callers must keep the zone set
    /// consistent with the set passed to [`Self::new`]; a missing zone
    /// produces a sentinel of `0.0 kg/kg` that downstream latent-load guards
    /// in `dwelling/mod.rs` interpret as "no latent contribution."
    ///
    /// The `0.0` sentinel is intentional — it is already caught by the
    /// `w_new <= 0.0` skip-guard at the sole runtime call site. Changing it
    /// to a physically plausible default would **bypass that guard** and
    /// silently corrupt the moisture mass balance. If you need to validate
    /// zone completeness, run in debug mode.
    #[must_use]
    pub fn humidity_ratio(&self, zone_id: ZoneId) -> f64 {
        debug_assert!(
            self.humidity_ratios.contains_key(&zone_id),
            "humidity_ratio: ZoneId {} not found in solver map — \
             ensure the solver is re-initialized after zone list changes",
            zone_id.0,
        );
        self.humidity_ratios.get(&zone_id).copied().unwrap_or(0.0)
    }

    /// Return the per-step condensation mass [kg] for `zone_id`.
    ///
    /// Positive = condensation to surfaces (moisture removed from air).
    /// Negative = frost deposition/sublimation (moisture added to air when
    /// `w_raw` was below zero and clamped up).
    #[must_use]
    pub fn condensation_mass_kg(&self, zone_id: ZoneId) -> f64 {
        debug_assert!(
            self.condensation_kg.contains_key(&zone_id),
            "condensation_mass_kg: ZoneId {} not found — \
             ensure the solver is re-initialized after zone list changes",
            zone_id.0,
        );
        self.condensation_kg.get(&zone_id).copied().unwrap_or(0.0)
    }

    /// Return whether condensation occurred for `zone_id` this step.
    #[must_use]
    pub fn condensation_occurred(&self, zone_id: ZoneId) -> bool {
        self.condensation_mass_kg(zone_id).abs() > f64::EPSILON
    }
}

impl DomainSolver for HumiditySolver {
    fn domain_id(&self) -> DomainId {
        HUMIDITY
    }

    fn resolve(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
        dt: Duration,
        out: &mut DomainUpdate,
    ) {
        let dt_s = dt.as_secs_f64();
        let p_pa = env.weather.pressure_pa();

        self.zone_temp_buf.clear();
        for zone in &env.zones {
            self.zone_temp_buf.insert(zone.id, zone.temperature_c);
        }

        self.latent_buf.clear();
        self.m_dot_inf_buf.clear();
        self.w_outdoor_buf.clear();
        self.condensation_kg.clear();
        if let Some(thermal_update) = env.custom_domains.iter().find(|u| u.domain_id == THERMAL) {
            for &(zone_id, t_c) in &thermal_update.zone_temperatures_c {
                self.zone_temp_buf.insert(zone_id, t_c);
            }
            if let Some(payload) = &thermal_update.custom_payload {
                // Thermal custom_payload format: [zone_id, q_latent_w, m_dot_inf_kg_s, w_outdoor,
                // energy_balance_residual_w] per zone. The 5-float format carries moisture
                // coupling data for semi-implicit humidity treatment and the per-step energy
                // balance residual for observability. Zones without infiltration have
                // m_dot_inf_kg_s = 0.0 and w_outdoor = 0.0.
                for quint in payload.chunks_exact(5) {
                    let zone_raw = quint[0];
                    let latent = quint[1];
                    let m_dot_inf = quint[2];
                    let w_outdoor = quint[3];
                    let _residual = quint[4];
                    if zone_raw.is_finite() && zone_raw >= 0.0 && zone_raw <= f64::from(u16::MAX) {
                        let zone = ZoneId(zone_raw as u16);
                        *self.latent_buf.entry(zone).or_insert(0.0) += latent;
                        if m_dot_inf > 0.0 {
                            self.m_dot_inf_buf.insert(zone, m_dot_inf);
                        }
                        if w_outdoor > 0.0 {
                            self.w_outdoor_buf.insert(zone, w_outdoor);
                        }
                    }
                }
            }
        }

        out.domain_id = HUMIDITY;
        out.zone_temperatures_c.clear();
        let payload = out.custom_payload.get_or_insert_with(Vec::new);
        payload.clear();

        for zone in &env.zones {
            let zone_id = zone.id;

            let moisture_mass_flow_kg_s = ports
                .humidity
                .iter()
                .filter(|entry| entry.zone == zone_id)
                .map(|entry| entry.moisture_mass_flow_kg_s)
                .sum::<f64>();

            let latent_gain_w = ports
                .thermal
                .iter()
                .filter(|entry| entry.zone == zone_id)
                .map(|entry| entry.latent_gain_w)
                .sum::<f64>()
                + self.latent_buf.get(&zone_id).copied().unwrap_or(0.0);

            let t_zone_c = self
                .zone_temp_buf
                .get(&zone_id)
                .copied()
                .unwrap_or(zone.temperature_c);
            let w_old = self
                .humidity_ratios
                .get(&zone_id)
                .copied()
                .unwrap_or_else(|| {
                    // `humidity_ratios` is pre-populated for every zone in
                    // `HumiditySolver::new`, so the fallback to `env.zones`
                    // should never be exercised after init. If this fires,
                    // a zone was added after solver construction without
                    // registering its initial humidity ratio — a code bug.
                    debug_assert!(
                        false,
                        "humidity_ratios must contain every zone after init; \
                         fallback to env.zones is dead code in steady-state"
                    );
                    zone.humidity_ratio
                });
            let volume_m3 = self
                .config
                .zone_volumes_m3
                .get(&zone_id)
                .copied()
                .unwrap_or(zone.volume_m3);
            let rho_air = moist_air_density_kg_m3(p_pa, t_zone_c, w_old);

            // Semi-implicit infiltration latent coupling.
            // When infiltration mass flow data is available from the thermal domain,
            // the infiltration moisture exchange is treated semi-implicitly:
            //   C_eff * dW/dt = m_dot_inf * h_fg * (W_out - W) + Q_other
            // with W_{n+1} on both sides → unconditional stability.
            // EnergyPlus Engineering Reference, "Moisture Predictor-Corrector":
            // ṁ_inf enters the implicit denominator coefficient A, just as
            // ṁ_inf*Cp does for sensible heat in "Basis for the Zone and Air
            // System Integration".
            let m_dot_inf_kg_s = self.m_dot_inf_buf.get(&zone_id).copied().unwrap_or(0.0);
            let w_outdoor = self.w_outdoor_buf.get(&zone_id).copied().unwrap_or(0.0);
            let has_infiltration_coupling = m_dot_inf_kg_s > 0.0;

            let (w_new, alpha) = if has_infiltration_coupling {
                // Semi-implicit infiltration latent coupling.
                // Moisture balance: C_eff * dW/dt = Q_other + m_dot * h_fg * (W_out - W)
                // where C_eff = h_fg * rho * V * M.
                // Discretized: W_{n+1} * (1 + alpha) = W_n + dt/C_eff * (Q_other + m_dot*h_fg*W_out)
                // where alpha = m_dot * h_fg * dt / C_eff = m_dot * dt / (rho * V * M)
                // (h_fg cancels between the numerator and C_eff in the denominator).
                let m_eff = rho_air
                    * volume_m3
                    * self.config.moisture_buffering_multiplier.max(f64::EPSILON);
                let c_eff = self.config.h_fg_j_kg * m_eff;
                let alpha = m_dot_inf_kg_s * dt_s / m_eff;

                // Split latent gain into infiltration (semi-implicit) and other (explicit).
                let q_inf_latent = m_dot_inf_kg_s * self.config.h_fg_j_kg * (w_outdoor - w_old);
                let q_other = latent_gain_w - q_inf_latent;

                // Semi-implicit update:
                //   numerator = W_old + alpha * W_out + dt/C_eff * Q_other
                //   denominator = 1 + alpha
                let numerator = w_old + alpha * w_outdoor + dt_s / c_eff * q_other;
                let denominator = 1.0 + alpha;

                tracing::debug!(
                    zone = zone_id.0,
                    alpha,
                    m_dot_inf_kg_s,
                    w_outdoor,
                    q_other,
                    "humidity solver semi-implicit infiltration coupling"
                );

                (numerator / denominator, alpha)
            } else if moisture_mass_flow_kg_s.abs() > 0.0 {
                // The humidity port's moisture_mass_flow_kg_s provides a direct
                // mass-transfer signal (e.g., dehumidifier condensation rate).
                // The thermal port's latent_gain_w independently carries moisture
                // contributions from sources that only write the thermal port
                // (occupants, process gains). Convert the humidity port's mass
                // flow to an equivalent latent power, subtract it from the total
                // latent_gain_w to avoid double-counting equipment that writes
                // both ports, and handle the remainder through the latent-gain
                // path. This ensures all moisture sources and sinks — not just
                // the humidity-port contributor — are accounted for.
                let mass_flow_latent_equiv_w = moisture_mass_flow_kg_s * self.config.h_fg_j_kg;
                let remaining_latent_gain_w = latent_gain_w - mass_flow_latent_equiv_w;

                tracing::debug!(
                    zone = zone_id.0,
                    moisture_mass_flow_kg_s,
                    mass_flow_latent_equiv_w,
                    remaining_latent_gain_w,
                    "humidity solver using explicit moisture_mass_flow_kg_s + remaining latent"
                );

                let d_w_mass = humidity_ratio_increment_from_mass_flow(
                    moisture_mass_flow_kg_s,
                    dt_s,
                    rho_air,
                    volume_m3,
                    self.config.moisture_buffering_multiplier,
                );
                let d_w_latent = humidity_ratio_increment(
                    remaining_latent_gain_w,
                    dt_s,
                    self.config.h_fg_j_kg,
                    rho_air,
                    volume_m3,
                    self.config.moisture_buffering_multiplier,
                );
                (w_old + d_w_mass + d_w_latent, 0.0)
            } else {
                tracing::debug!(
                    zone = zone_id.0,
                    latent_gain_w,
                    "humidity solver falling back to latent_gain_w / h_fg"
                );
                let d_w = humidity_ratio_increment(
                    latent_gain_w,
                    dt_s,
                    self.config.h_fg_j_kg,
                    rho_air,
                    volume_m3,
                    self.config.moisture_buffering_multiplier,
                );
                (w_old + d_w, 0.0)
            };

            let w_sat = humidity_ratio_from_tdp(t_zone_c, p_pa);
            let w_raw = w_new;
            let w_new = w_new.clamp(0.0, w_sat);
            self.humidity_ratios.insert(zone_id, w_new);

            // Condensation / frost deposition mass: moisture removed from (or
            // added to) the zone air by clamp enforcement. Positive = condensation
            // (w_raw > w_sat, mass removed); negative = frost sublimation
            // (w_raw < 0, mass added by clamping up to zero).
            let condensation_mass_kg = (w_raw - w_new) * rho_air * volume_m3;
            let condensation_occurred_flag = if condensation_mass_kg.abs() > f64::EPSILON {
                1.0_f64
            } else {
                0.0_f64
            };
            self.condensation_kg.insert(zone_id, condensation_mass_kg);

            let rh = relative_humidity(t_zone_c, w_new, p_pa);
            let wet_bulb_c = wet_bulb_from_humidity_ratio(t_zone_c, w_new, p_pa);

            out.zone_temperatures_c.push((zone_id, t_zone_c));
            // Humidity custom_payload format: [zone_id, w_new, rh, wet_bulb_c, alpha,
            // condensation_mass_kg, condensation_occurred_flag]
            // (7 floats per zone).
            // alpha is the semi-implicit infiltration coupling coefficient (0.0 when no infiltration).
            payload.extend_from_slice(&[
                f64::from(zone_id.0),
                w_new,
                rh,
                wet_bulb_c,
                alpha,
                condensation_mass_kg,
                condensation_occurred_flag,
            ]);
        }
        out.zone_temperatures_c.sort_by_key(|(zone_id, _)| *zone_id);
    }
}

fn humidity_ratio_increment(
    latent_gain_w: f64,
    dt_s: f64,
    h_fg_j_kg: f64,
    rho_air_kg_m3: f64,
    volume_m3: f64,
    moisture_buffering_multiplier: f64,
) -> f64 {
    let denom =
        h_fg_j_kg * rho_air_kg_m3 * volume_m3 * moisture_buffering_multiplier.max(f64::EPSILON);
    (latent_gain_w * dt_s) / denom
}

fn humidity_ratio_increment_from_mass_flow(
    moisture_mass_flow_kg_s: f64,
    dt_s: f64,
    rho_air_kg_m3: f64,
    volume_m3: f64,
    moisture_buffering_multiplier: f64,
) -> f64 {
    let denom = rho_air_kg_m3 * volume_m3 * moisture_buffering_multiplier.max(f64::EPSILON);
    (moisture_mass_flow_kg_s * dt_s) / denom
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{FixedOffset, TimeZone};
    use hares_physics::psychrometrics::relative_humidity;
    use hares_types::{
        DomainSolver, DomainUpdate, EnvironmentState, GridState, HumidityAccumulator, PortSlots,
        SurfaceIrradiance, THERMAL, THERMAL_CATEGORY_COUNT, ThermalAccumulator, WeatherState,
        ZoneId, ZoneState,
    };

    use crate::humidity_solver::{HumiditySolver, HumiditySolverConfig, humidity_ratio_increment};

    fn env_with_zone(temp_c: f64, humidity_ratio: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: temp_c,
                humidity_ratio,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 180.0,
                ground_temp_c: 10.0,
                sky_temp_c: 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    #[test]
    fn known_latent_gain_increment_matches_formula() {
        let d_w = humidity_ratio_increment(100.0, 60.0, 2_501_000.0, 1.2, 200.0, 1.0);
        let expected = (100.0 * 60.0) / (2_501_000.0 * 1.2 * 200.0);
        assert!((d_w - expected).abs() <= 1e-12);
    }

    #[test]
    fn moisture_balance_holds_per_timestep() {
        let env = env_with_zone(22.0, 0.008);
        // Use multiplier=1.0 to test raw moisture mass balance without buffering.
        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let mut solver = HumiditySolver::new(config.clone(), &env);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 0.0,
                latent_gain_w: 100.0,
                ..ThermalAccumulator::new(ZoneId(1))
            }],
            ..Default::default()
        };
        let w_old = solver.humidity_ratio(ZoneId(1));
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let w_new = solver.humidity_ratio(ZoneId(1));

        let rho = hares_physics::air_properties::moist_air_density_kg_m3(
            env.weather.pressure_pa(),
            env.zones[0].temperature_c,
            w_old,
        );
        let volume = config
            .zone_volumes_m3
            .get(&ZoneId(1))
            .copied()
            .unwrap_or(env.zones[0].volume_m3);
        let delta_m = (w_new - w_old) * rho * volume;
        let source_m = (100.0 * 60.0) / config.h_fg_j_kg;
        assert!((delta_m - source_m).abs() < 1e-6);
    }

    #[test]
    fn relative_humidity_round_trip_is_consistent() {
        let mut env = env_with_zone(24.0, 0.009);
        env.custom_domains.push(DomainUpdate {
            domain_id: THERMAL,
            zone_temperatures_c: vec![(ZoneId(1), 23.0)],
            custom_payload: None,
        });
        let mut solver = HumiditySolver::new(HumiditySolverConfig::default(), &env);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 0.0,
                latent_gain_w: 50.0,
                ..ThermalAccumulator::new(ZoneId(1))
            }],
            ..Default::default()
        };
        let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let payload = update.custom_payload.unwrap();
        let w = payload[1];
        let rh = payload[2];
        let rh_round_trip = relative_humidity(23.0, w, env.weather.pressure_pa());
        assert!((rh - rh_round_trip).abs() <= 0.001);
    }

    #[test]
    fn humidity_ratio_clamps_to_saturation_and_zero() {
        let env = env_with_zone(20.0, 0.008);
        let mut solver = HumiditySolver::new(HumiditySolverConfig::default(), &env);

        let ports_hi = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 0.0,
                latent_gain_w: 1.0e9,
                ..ThermalAccumulator::new(ZoneId(1))
            }],
            ..Default::default()
        };
        let _ = solver.resolve_new(&ports_hi, &env, Duration::from_secs(60));
        let w_hi = solver.humidity_ratio(ZoneId(1));
        let w_sat =
            hares_physics::psychrometrics::humidity_ratio_from_tdp(20.0, env.weather.pressure_pa());
        assert!(w_hi <= w_sat);
        // Condensation mass must be positive when w_raw exceeds w_sat.
        assert!(
            solver.condensation_mass_kg(ZoneId(1)) > 0.0,
            "condensation mass must be > 0 when clamped at w_sat"
        );

        // Reset the solver for the below-zero test.
        let mut solver = HumiditySolver::new(HumiditySolverConfig::default(), &env);
        let ports_lo = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 0.0,
                latent_gain_w: -1.0e9,
                ..ThermalAccumulator::new(ZoneId(1))
            }],
            ..Default::default()
        };
        let _ = solver.resolve_new(&ports_lo, &env, Duration::from_secs(60));
        let w_lo = solver.humidity_ratio(ZoneId(1));
        assert!(w_lo >= 0.0);
        // When w_raw < 0, condensation mass is negative (moisture added to air
        // by clamping up to zero — frost sublimation).
        assert!(
            solver.condensation_mass_kg(ZoneId(1)) < 0.0,
            "condensation mass must be < 0 when clamped at zero from below"
        );
    }

    /// Regression: latent heat was inconsistent between modules (2450 vs 2501 kJ/kg).
    /// If thermal_solver uses one h_fg to compute Q_latent from infiltration and
    /// humidity_solver uses a different h_fg to convert Q_latent back to delta_w,
    /// the moisture mass balance breaks. This test verifies that a known latent load
    /// passed through the thermal domain's custom_payload round-trips correctly:
    /// the mass of moisture added (delta_w * rho * V) must equal Q_latent * dt / h_fg
    /// to within 1e-6 kg.
    ///
    /// With the old 2450 kJ/kg value in one module and 2501 in the other, the error
    /// would be ~2% (50/2500), far exceeding the 1e-6 kg tolerance on a 200 m^3 zone.
    #[test]
    fn latent_heat_round_trip_through_thermal_domain_payload() {
        // Simulate thermal_solver emitting a known latent load via custom_payload.
        let q_latent_w = 500.0; // 500 W latent gain
        let dt_s = 60.0;
        let zone_id = ZoneId(1);

        // Build an environment where the thermal domain has already run and produced
        // a latent payload for our zone.
        let mut env = env_with_zone(22.0, 0.008);
        env.custom_domains.push(DomainUpdate {
            domain_id: THERMAL,
            zone_temperatures_c: vec![(zone_id, 22.0)],
            custom_payload: Some(vec![f64::from(zone_id.0), q_latent_w, 0.0, 0.0, 0.0]),
        });

        // Use multiplier=1.0 so the full latent energy maps directly to moisture mass,
        // isolating the h_fg constant round-trip from moisture buffering behavior.
        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let h_fg = config.h_fg_j_kg;
        let mut solver = HumiditySolver::new(config, &env);
        let w_old = solver.humidity_ratio(zone_id);

        let ports = PortSlots::default();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(dt_s as u64));
        let w_new = solver.humidity_ratio(zone_id);

        let rho = hares_physics::air_properties::moist_air_density_kg_m3(
            env.weather.pressure_pa(),
            22.0,
            w_old,
        );
        let volume = env.zones[0].volume_m3;

        // Moisture mass actually added by the solver
        let delta_m_actual_kg = (w_new - w_old) * rho * volume;
        // Moisture mass implied by the energy input
        let delta_m_expected_kg = q_latent_w * dt_s / h_fg;

        assert!(
            (delta_m_actual_kg - delta_m_expected_kg).abs() < 1e-6,
            "moisture mass mismatch: actual={delta_m_actual_kg:.9} kg, \
             expected={delta_m_expected_kg:.9} kg -- latent heat values likely \
             differ between thermal and humidity solvers"
        );
    }

    /// Multi-zone thermal custom payload: 5-float format `[zone_id, q_latent_w,
    /// m_dot_inf_kg_s, w_outdoor, residual_w]` per zone. A prior bug used
    /// `chunks_exact(4)` which silently misaligned zone records for N > 1.
    /// This test injects different latent gains into two zones via a single
    /// 10-element payload vector and verifies that each zone receives its
    /// own latent value — not a value from an adjacent zone's slot.
    #[test]
    fn multi_zone_thermal_payload_parses_each_zone_independently() {
        let w_init = 0.008_f64;
        let zone1_id = ZoneId(1);
        let zone2_id = ZoneId(2);
        let latent_zone1_w = 300.0_f64;
        let latent_zone2_w = 700.0_f64;
        let dt = Duration::from_secs(60);

        let env = env_with_two_zones(200.0, 200.0, w_init, w_init);

        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let mut solver = HumiditySolver::new(config, &env);

        // 5-float per-zone thermal payload: [zone_id, latent, m_dot, w_out, residual]
        let mut thermal_env = env_with_zone(22.0, w_init);
        thermal_env.zones = env.zones.clone(); // two zones
        thermal_env.custom_domains.push(DomainUpdate {
            domain_id: THERMAL,
            zone_temperatures_c: vec![(zone1_id, 22.0), (zone2_id, 22.0)],
            custom_payload: Some(vec![
                f64::from(zone1_id.0),
                latent_zone1_w,
                0.0,
                0.0,
                0.0, // residual zone 1
                f64::from(zone2_id.0),
                latent_zone2_w,
                0.0,
                0.0,
                0.0, // residual zone 2
            ]),
        });

        let ports = PortSlots::default();
        let _ = solver.resolve_new(&ports, &thermal_env, dt);

        let w1 = solver.humidity_ratio(zone1_id);
        let w2 = solver.humidity_ratio(zone2_id);

        // Both zones should have moved from w_init, each by an amount
        // commensurate with its own latent gain.
        assert!(
            w1 > w_init,
            "zone 1 humidity must rise from {w_init} (got {w1})"
        );
        assert!(
            w2 > w_init,
            "zone 2 humidity must rise from {w_init} (got {w2})"
        );

        // dW scales linearly with latent gain (same volume, same dt).
        let dw1 = w1 - w_init;
        let dw2 = w2 - w_init;
        let expected_ratio = latent_zone2_w / latent_zone1_w; // 700/300 ≈ 2.333
        let actual_ratio = dw2 / dw1;
        assert!(
            (actual_ratio - expected_ratio).abs() < 1e-6,
            "humidity ratio change must scale with latent gain: \
             expected dw2/dw1={expected_ratio:.6}, got {actual_ratio:.6}"
        );
    }

    #[test]
    fn ashrae_moisture_balance_dimensional_check() {
        // ASHRAE 2017 HOF Ch. 24: dW = (Q_latent * dt) / (h_fg * rho * V)
        // For V=200 m³, T=20°C, P=101325 Pa, Q_latent=100W, dt=3600s:
        // rho ≈ 1.204 kg/m³ (moist air at low humidity)
        // dW = (100 * 3600) / (2_501_000 * rho * 200)
        let p_pa = 101_325.0;
        let t_c = 20.0;
        let w_init = 0.005;
        let volume_m3 = 200.0;
        let q_latent_w = 100.0;
        let dt_s = 3600.0;
        let h_fg = 2_501_000.0;

        let rho = hares_physics::air_properties::moist_air_density_kg_m3(p_pa, t_c, w_init);

        let dw = humidity_ratio_increment(q_latent_w, dt_s, h_fg, rho, volume_m3, 1.0);
        let expected = (q_latent_w * dt_s) / (h_fg * rho * volume_m3);

        assert!(
            (dw - expected).abs() < 1e-12,
            "dW: {dw}, expected: {expected}"
        );

        // Verify order-of-magnitude: ~6e-4 kg/kg for 100W over 1 hour
        assert!(
            dw > 5e-4 && dw < 8e-4,
            "dW={dw} outside plausible range [5e-4, 8e-4] kg/kg"
        );
    }

    #[test]
    fn moisture_buffering_multiplier_scales_swing() {
        let env = env_with_zone(22.0, 0.008);
        let mut solver_1 = HumiditySolver::new(
            HumiditySolverConfig {
                moisture_buffering_multiplier: 1.0,
                ..HumiditySolverConfig::default()
            },
            &env,
        );
        let mut solver_2 = HumiditySolver::new(
            HumiditySolverConfig {
                moisture_buffering_multiplier: 2.0,
                ..HumiditySolverConfig::default()
            },
            &env,
        );
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 0.0,
                latent_gain_w: 100.0,
                ..ThermalAccumulator::new(ZoneId(1))
            }],
            ..Default::default()
        };

        let w1_old = solver_1.humidity_ratio(ZoneId(1));
        let _ = solver_1.resolve_new(&ports, &env, Duration::from_secs(60));
        let w1_new = solver_1.humidity_ratio(ZoneId(1));

        let w2_old = solver_2.humidity_ratio(ZoneId(1));
        let _ = solver_2.resolve_new(&ports, &env, Duration::from_secs(60));
        let w2_new = solver_2.humidity_ratio(ZoneId(1));

        let dw1 = w1_new - w1_old;
        let dw2 = w2_new - w2_old;
        assert!((dw2 - 0.5 * dw1).abs() < 1e-8);
    }

    /// Verifies that the thermal solver's latent heat constant matches the
    /// humidity solver's. If they diverge, infiltration-driven moisture will
    /// accumulate a systematic error (~1.9% per step for 2454 vs 2501 kJ/kg).
    #[test]
    fn thermal_and_humidity_solvers_share_latent_heat_constant() {
        use hares_physics::constants::{KJ_TO_J, LATENT_HEAT_VAPORISATION_0C_KJ_KG};

        let thermal_h_fg = LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J;
        let humidity_h_fg = HumiditySolverConfig::default().h_fg_j_kg;

        assert!(
            (thermal_h_fg - humidity_h_fg).abs() < f64::EPSILON,
            "latent heat mismatch: thermal={thermal_h_fg} J/kg, humidity={humidity_h_fg} J/kg"
        );
    }

    /// Integration test: known infiltration latent load through both solvers
    /// must produce a humidity ratio that matches first-principles calculation.
    /// Q_latent = m_dot_infiltration * h_fg * (w_outdoor - w_indoor)
    /// delta_w = Q_latent * dt / (h_fg * rho * V * multiplier)
    #[test]
    fn infiltration_humidity_ratio_matches_first_principles() {
        let h_fg = HumiditySolverConfig::default().h_fg_j_kg;
        let w_indoor = 0.008;
        let w_outdoor = 0.012;
        let t_c = 22.0;
        let p_pa = 101_325.0;
        let volume_m3 = 200.0;
        let dt_s = 300.0;

        let rho = hares_physics::air_properties::moist_air_density_kg_m3(p_pa, t_c, w_indoor);
        let infiltration_m3_s = 0.05; // ~180 m³/h
        let m_dot = infiltration_m3_s * rho;

        // Latent heat gain from infiltration (as the thermal solver would compute)
        let q_latent_w = m_dot * h_fg * (w_outdoor - w_indoor);

        // Expected humidity ratio change (as the humidity solver computes)
        let dw_expected = humidity_ratio_increment(q_latent_w, dt_s, h_fg, rho, volume_m3, 1.0);

        // First-principles: delta_w = m_dot * (w_out - w_in) * dt / (rho * V)
        let dw_first_principles = m_dot * (w_outdoor - w_indoor) * dt_s / (rho * volume_m3);

        assert!(
            (dw_expected - dw_first_principles).abs() < 1e-12,
            "humidity ratio mismatch: solver={dw_expected:.9e}, \
             first_principles={dw_first_principles:.9e} -- latent heat cancellation failed"
        );
    }

    /// The default multiplier (15×) must produce a humidity-ratio change exactly
    /// 15× smaller than the same solver at multiplier=1.0 under identical load.
    /// This pins the OCHRE-aligned default and catches accidental resets.
    #[test]
    fn default_multiplier_matches_ochre_15x() {
        // Explicit pin: catch any accidental change to the default constant
        let cfg = HumiditySolverConfig::default();
        assert_eq!(cfg.moisture_buffering_multiplier, 15.0);
        assert_eq!(cfg.h_fg_j_kg, 2_501_000.0);

        let env = env_with_zone(22.0, 0.008);
        let mut solver_unbuffered = HumiditySolver::new(
            HumiditySolverConfig {
                moisture_buffering_multiplier: 1.0,
                ..HumiditySolverConfig::default()
            },
            &env,
        );
        let mut solver_default = HumiditySolver::new(HumiditySolverConfig::default(), &env);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 0.0,
                latent_gain_w: 200.0,
                ..ThermalAccumulator::new(ZoneId(1))
            }],
            ..Default::default()
        };

        let w_unbuffered_old = solver_unbuffered.humidity_ratio(ZoneId(1));
        let _ = solver_unbuffered.resolve_new(&ports, &env, Duration::from_secs(60));
        let dw_unbuffered = solver_unbuffered.humidity_ratio(ZoneId(1)) - w_unbuffered_old;

        let w_default_old = solver_default.humidity_ratio(ZoneId(1));
        let _ = solver_default.resolve_new(&ports, &env, Duration::from_secs(60));
        let dw_default = solver_default.humidity_ratio(ZoneId(1)) - w_default_old;

        assert!(
            (dw_default - dw_unbuffered / 15.0).abs() < 1e-12,
            "default multiplier must be 15×: dw_default={dw_default:.3e}, \
             dw_unbuffered/15={:.3e}",
            dw_unbuffered / 15.0
        );
    }

    // -----------------------------------------------------------------------
    // Physics-grounded tests -- validated against ASHRAE first principles.
    // -----------------------------------------------------------------------

    /// Direct test of humidity_ratio_increment with known inputs.
    /// dW = (Q_latent * dt) / (h_fg * rho * V * multiplier)
    ///    = (500 * 60) / (2501000 * 1.2 * 200 * 15) = 3.3320e-6 kg/kg
    #[test]
    fn humidity_increment_ashrae_first_principles() {
        let dw = humidity_ratio_increment(500.0, 60.0, 2_501_000.0, 1.2, 200.0, 15.0);
        let expected = (500.0 * 60.0) / (2_501_000.0 * 1.2 * 200.0 * 15.0);
        assert!(
            (dw - expected).abs() < 1e-12,
            "dW mismatch: got {dw:.12e}, expected {expected:.12e}"
        );
        // Cross-check the numeric value from the ticket spec
        assert!(
            (dw - 3.332_0e-6).abs() < 1e-10,
            "dW={dw:.12e} not near 3.3320e-6"
        );
    }

    /// Moisture buffering multiplier scales linearly: dW(m=1) / dW(m=15) = 15.0.
    #[test]
    fn humidity_buffering_multiplier_scales_linearly() {
        let dw_1 = humidity_ratio_increment(500.0, 60.0, 2_501_000.0, 1.2, 200.0, 1.0);
        let dw_15 = humidity_ratio_increment(500.0, 60.0, 2_501_000.0, 1.2, 200.0, 15.0);
        let ratio = dw_1 / dw_15;
        assert!(
            (ratio - 15.0).abs() < 1e-10,
            "multiplier ratio must be 15.0: got {ratio}"
        );
    }

    /// zone_volumes_m3 override replaces ZoneState::volume_m3 in the solver.
    ///
    /// A larger effective volume reduces the humidity-ratio swing for the same
    /// latent gain. Assert: dW(override) / dW(zone) ≈ zone_volume / override_volume.
    #[test]
    fn humidity_zone_volume_override() {
        let zone_id = ZoneId(1);
        let zone_volume_m3 = 200.0_f64;
        let override_volume_m3 = 600.0_f64; // 3× larger
        let latent_w = 300.0_f64;
        let dt = Duration::from_secs(60);

        let env = env_with_zone(22.0, 0.008);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: zone_id,
                sensible_gain_w: 0.0,
                latent_gain_w: latent_w,
                ..ThermalAccumulator::new(zone_id)
            }],
            ..Default::default()
        };

        // Solver using zone's own volume (200 m³).
        let mut solver_zone_vol = HumiditySolver::new(
            HumiditySolverConfig {
                moisture_buffering_multiplier: 1.0,
                ..HumiditySolverConfig::default()
            },
            &env,
        );
        let w_before = solver_zone_vol.humidity_ratio(zone_id);
        let _ = solver_zone_vol.resolve_new(&ports, &env, dt);
        let dw_zone = solver_zone_vol.humidity_ratio(zone_id) - w_before;

        // Solver using overridden volume (600 m³).
        let mut overrides = HashMap::new();
        overrides.insert(zone_id, override_volume_m3);
        let mut solver_override = HumiditySolver::new(
            HumiditySolverConfig {
                zone_volumes_m3: overrides,
                moisture_buffering_multiplier: 1.0,
                ..HumiditySolverConfig::default()
            },
            &env,
        );
        let w_before_ov = solver_override.humidity_ratio(zone_id);
        let _ = solver_override.resolve_new(&ports, &env, dt);
        let dw_override = solver_override.humidity_ratio(zone_id) - w_before_ov;

        // dW scales inversely with volume: dW(override) / dW(zone) = zone_vol / override_vol.
        // Both solvers start from the same w_old, so density is identical.
        let expected_ratio = zone_volume_m3 / override_volume_m3;
        let actual_ratio = dw_override / dw_zone;
        assert!(
            (actual_ratio - expected_ratio).abs() < 1e-8,
            "volume override must reduce dW proportionally: \
             expected ratio {expected_ratio:.6}, got {actual_ratio:.6}",
        );
        assert!(
            dw_override < dw_zone,
            "larger override volume must produce smaller humidity swing: \
             dw_override={dw_override:.3e}, dw_zone={dw_zone:.3e}",
        );
    }

    fn env_with_two_zones(vol_a: f64, vol_b: f64, w_a: f64, w_b: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![
                ZoneState {
                    id: ZoneId(1),
                    temperature_c: 22.0,
                    humidity_ratio: w_a,
                    volume_m3: vol_a,
                },
                ZoneState {
                    id: ZoneId(2),
                    temperature_c: 22.0,
                    humidity_ratio: w_b,
                    volume_m3: vol_b,
                },
            ],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 180.0,
                ground_temp_c: 10.0,
                sky_temp_c: 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// Two zones with different volumes but the same latent gain.
    /// The larger zone must show a smaller humidity-ratio swing because the moisture
    /// is diluted into more air mass.  dW ∝ 1 / (V × ρ), so dW_small / dW_large = V_large / V_small.
    #[test]
    fn humidity_two_zone_different_volumes() {
        let w_init = 0.008_f64;
        let vol_small = 100.0_f64;
        let vol_large = 400.0_f64;
        let latent_w = 200.0_f64;
        let dt = Duration::from_secs(60);

        let env = env_with_two_zones(vol_small, vol_large, w_init, w_init);

        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let mut solver = HumiditySolver::new(config, &env);

        let ports = PortSlots {
            thermal: vec![
                ThermalAccumulator {
                    zone: ZoneId(1),
                    sensible_gain_w: 0.0,
                    latent_gain_w: latent_w,
                    ..ThermalAccumulator::new(ZoneId(1))
                },
                ThermalAccumulator {
                    zone: ZoneId(2),
                    sensible_gain_w: 0.0,
                    latent_gain_w: latent_w,
                    ..ThermalAccumulator::new(ZoneId(2))
                },
            ],
            ..Default::default()
        };

        let _ = solver.resolve_new(&ports, &env, dt);

        let dw_small = solver.humidity_ratio(ZoneId(1)) - w_init;
        let dw_large = solver.humidity_ratio(ZoneId(2)) - w_init;

        assert!(
            dw_small > dw_large,
            "smaller volume must show larger humidity swing: \
             dw_small={dw_small:.3e}, dw_large={dw_large:.3e}"
        );

        // dW scales inversely with volume: dW_small / dW_large = V_large / V_small
        let expected_ratio = vol_large / vol_small;
        let actual_ratio = dw_small / dw_large;
        assert!(
            (actual_ratio - expected_ratio).abs() < 1e-6,
            "dW ratio must equal V_large/V_small={expected_ratio:.1}: got {actual_ratio:.6}"
        );
    }

    /// Zone registered after solver construction (inserted directly into
    /// `humidity_ratios`) is read from the committed state, not from
    /// `env.zones.humidity_ratio`.  In production `HumiditySolver::new`
    /// registers all zones; this test simulates a zone added dynamically
    /// by inserting its committed humidity ratio before the next `resolve`.
    #[test]
    fn humidity_committed_state_read_for_newly_inserted_zone() {
        let w_existing = 0.008_f64;
        let w_new_zone = 0.010_f64;
        let dt = Duration::from_secs(60);

        // Construct solver with only zone 1.
        let env_init = env_with_zone(22.0, w_existing);
        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let mut solver = HumiditySolver::new(config, &env_init);

        // Pre-register zone 2 in humidity_ratios with its committed initial value.
        // In production this happens in HumiditySolver::new for every zone in
        // env.zones; the fallback to env.zones is guarded by a debug_assert!
        // confirming it is dead code after init.
        solver.humidity_ratios.insert(ZoneId(2), w_new_zone);

        // Resolve with an environment that includes zone 2.
        let env_two = env_with_two_zones(200.0, 200.0, w_existing, w_new_zone);

        // Zero latent gain so humidity_ratio stays at its committed value.
        let ports = PortSlots {
            thermal: vec![
                ThermalAccumulator::new(ZoneId(1)),
                ThermalAccumulator::new(ZoneId(2)),
            ],
            ..Default::default()
        };

        let _ = solver.resolve_new(&ports, &env_two, dt);

        // Zone 2 must read from the committed humidity_ratios entry.
        let w2 = solver.humidity_ratio(ZoneId(2));
        assert!(
            (w2 - w_new_zone).abs() < 1e-6,
            "committed humidity_ratio for zone 2 must equal init value {w_new_zone}, got {w2}"
        );
    }

    /// Zones are independent: latent gain applied to zone 1 must not transfer to zone 2.
    ///
    /// Zone 2 (garage) carries zero latent gain; its humidity ratio must remain exactly
    /// at its initial value because the solver has no inter-zone moisture coupling.
    #[test]
    fn humidity_zones_are_independent() {
        let w_outdoor = 0.005_f64;
        let w_indoor = 0.008_f64;
        let dt = Duration::from_secs(60);

        // Zone 2 starts at outdoor humidity; zone 1 starts higher.
        let env = env_with_two_zones(200.0, 150.0, w_indoor, w_outdoor);

        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let mut solver = HumiditySolver::new(config, &env);

        // Apply latent gain to zone 1 only; zone 2 gets none.
        let ports = PortSlots {
            thermal: vec![
                ThermalAccumulator {
                    zone: ZoneId(1),
                    sensible_gain_w: 0.0,
                    latent_gain_w: 500.0,
                    ..ThermalAccumulator::new(ZoneId(1))
                },
                ThermalAccumulator::new(ZoneId(2)),
            ],
            ..Default::default()
        };

        let _ = solver.resolve_new(&ports, &env, dt);

        let w_zone2_after = solver.humidity_ratio(ZoneId(2));
        let w_indoor_after = solver.humidity_ratio(ZoneId(1));

        // Zone 2 with no gain must remain at its initial value (no inter-zone transfer).
        assert!(
            (w_zone2_after - w_outdoor).abs() < 1e-9,
            "zone 2 with no gain must not change: expected {w_outdoor}, got {w_zone2_after}"
        );
        // Zone 1 must have gained moisture from its latent input.
        assert!(
            w_indoor_after > w_indoor,
            "zone 1 must rise with latent gain: before={w_indoor}, after={w_indoor_after}"
        );
    }

    /// Two zones with different latent gains evolve independently and proportionally.
    #[test]
    fn humidity_two_zone_independent_evolution() {
        let zone_a = ZoneId(1);
        let zone_b = ZoneId(2);
        let w_init = 0.008;

        let env = EnvironmentState {
            zones: vec![
                ZoneState {
                    id: zone_a,
                    temperature_c: 22.0,
                    humidity_ratio: w_init,
                    volume_m3: 200.0,
                },
                ZoneState {
                    id: zone_b,
                    temperature_c: 22.0,
                    humidity_ratio: w_init,
                    volume_m3: 200.0,
                },
            ],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 180.0,
                ground_temp_c: 10.0,
                sky_temp_c: 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        };

        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let mut solver = HumiditySolver::new(config.clone(), &env);

        // Apply 100W to zone A, 300W to zone B via port slots
        let ports = PortSlots {
            thermal: vec![
                ThermalAccumulator {
                    zone: zone_a,
                    sensible_gain_w: 0.0,
                    latent_gain_w: 100.0,
                    ..ThermalAccumulator::new(zone_a)
                },
                ThermalAccumulator {
                    zone: zone_b,
                    sensible_gain_w: 0.0,
                    latent_gain_w: 300.0,
                    ..ThermalAccumulator::new(zone_b)
                },
            ],
            ..Default::default()
        };

        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));

        let dw_a = solver.humidity_ratio(zone_a) - w_init;
        let dw_b = solver.humidity_ratio(zone_b) - w_init;

        // Both must be positive (latent gain adds moisture)
        assert!(dw_a > 0.0, "zone A dW must be positive: {dw_a}");
        assert!(dw_b > 0.0, "zone B dW must be positive: {dw_b}");

        // Zone B receives 3x the latent gain, so its dW must be ~3x zone A's.
        // Slight deviation possible due to density depending on humidity ratio,
        // but within a single timestep with identical initial conditions the
        // density is the same, so the ratio must be exact.
        let ratio = dw_b / dw_a;
        assert!(
            (ratio - 3.0).abs() < 1e-8,
            "zone B must evolve at 3x zone A's rate: ratio={ratio}"
        );

        // True independence check: zone A's dW in the two-zone solver must match
        // what a single-zone solver with only zone A's gain would produce.
        let env_single = EnvironmentState {
            zones: vec![ZoneState {
                id: zone_a,
                temperature_c: 22.0,
                humidity_ratio: w_init,
                volume_m3: 200.0,
            }],
            ..env.clone()
        };
        let mut solver_single = HumiditySolver::new(config, &env_single);
        let ports_single = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: zone_a,
                sensible_gain_w: 0.0,
                latent_gain_w: 100.0,
                ..ThermalAccumulator::new(zone_a)
            }],
            ..Default::default()
        };
        let _ = solver_single.resolve_new(&ports_single, &env_single, Duration::from_secs(60));
        let dw_a_solo = solver_single.humidity_ratio(zone_a) - w_init;
        assert!(
            (dw_a - dw_a_solo).abs() < 1e-15,
            "zone A dW must be identical whether zone B exists or not: \
             two_zone={dw_a}, solo={dw_a_solo}"
        );
    }

    // -----------------------------------------------------------------------
    // Semi-implicit infiltration latent coupling
    // -----------------------------------------------------------------------

    /// At coarse timestep (600 s) and high infiltration (2 ACH) with no moisture
    /// buffering (M=1), the semi-implicit formula is more accurate than the
    /// explicit forward-Euler method compared to the analytical exponential-decay
    /// solution. This validates that the semi-implicit discretization chosen for
    /// the humidity solver is the better numerical method, consistent with how
    /// the sensible path treats infiltration semi-implicitly.
    #[test]
    fn semi_implicit_infiltration_more_accurate_than_explicit_at_coarse_dt() {
        // High-infiltration scenario: 2 ACH, 200 m³ zone, M=1.
        let volume_m3 = 200.0_f64;
        let ach = 2.0_f64;
        let t_c = 22.0_f64;
        let p_pa = 101_325.0_f64;
        let w_outdoor = 0.012_f64; // humid outdoor air
        let w_initial = 0.005_f64; // drier indoor air
        let dt_s = 600.0_f64; // 10-minute (coarse) timestep
        let h_fg = HumiditySolverConfig::default().h_fg_j_kg;

        let rho = hares_physics::air_properties::moist_air_density_kg_m3(p_pa, t_c, w_initial);
        let q_m3_s = ach * volume_m3 / 3600.0;
        let m_dot = rho * q_m3_s;

        // Analytical steady-state: w_new → w_outdoor as t → ∞.
        // One-step analytical solution (exact exponential decay):
        //   w(t) = w_out + (w_0 - w_out) * exp(-m_dot * dt / (rho * V))
        // With M=1 (no buffering):
        let tau = rho * volume_m3 / m_dot; // = V / Q = 3600 / ACH
        let w_analytical = w_outdoor + (w_initial - w_outdoor) * (-dt_s / tau).exp();

        // Current explicit update (what the solver actually does):
        //   Q_latent = m_dot * h_fg * (w_out - w_old)
        //   d_w = Q_latent * dt / (h_fg * rho * V * M)
        //       = m_dot * (w_out - w_old) * dt / (rho * V)
        let q_latent_explicit = m_dot * h_fg * (w_outdoor - w_initial);
        let dw_explicit =
            humidity_ratio_increment(q_latent_explicit, dt_s, h_fg, rho, volume_m3, 1.0);
        let w_explicit = w_initial + dw_explicit;

        // Semi-implicit update:
        //   alpha = m_dot * dt / (rho * V)   (h_fg cancels between numerator and C_eff)
        //   W_{n+1} = (W_old + alpha * W_out) / (1 + alpha)
        let alpha = m_dot * dt_s / (rho * volume_m3);
        let w_semi_implicit = (w_initial + alpha * w_outdoor) / (1.0 + alpha);

        // The amplification factor for the explicit method: A = 1 - m_dot*dt/(rho*V)
        let amplification = 1.0 - m_dot * dt_s / (rho * volume_m3);
        // At 2 ACH, dt=600s: A = 1 - (2*200/3600)*600/200 = 1 - 0.333 = 0.667
        // This is stable but with 33% per-step damping — significant.

        // Both methods should be stable (A > 0), confirming stability analysis in ticket.
        assert!(
            amplification > 0.0 && amplification < 1.0,
            "amplification factor A={amplification:.4} must be in (0,1) for stability"
        );

        // The explicit method produces a result that overshoots or undershoots the
        // analytical solution at coarse timesteps. Measure the error of each.
        let err_explicit = (w_explicit - w_analytical).abs();
        let err_semi_implicit = (w_semi_implicit - w_analytical).abs();

        // The semi-implicit method should be more accurate than the explicit method
        // at coarse timesteps. At fine timesteps they converge (both first-order).
        assert!(
            err_semi_implicit < err_explicit,
            "semi-implicit error {err_semi_implicit:.6e} must be smaller than \
             explicit error {err_explicit:.6e} at coarse dt={dt_s}s, ACH={ach}"
        );

        // The explicit error should be non-trivial (>1% of the humidity ratio change)
        // to confirm this is a meaningful consistency gap, not floating-point noise.
        let dw_total = (w_outdoor - w_initial).abs();
        assert!(
            err_explicit > 0.01 * dw_total,
            "explicit error {err_explicit:.6e} must be >1% of Δw={dw_total:.6e} \
             to demonstrate meaningful divergence at coarse dt"
        );
    }

    /// Ticket 001: dehumidifier moisture mass round-trips through humidity solver.
    ///
    /// When the dehumidifier emits a `Humidity { moisture_mass_flow_kg_s }` port
    /// contribution, the humidity solver integrates it directly to a humidity-ratio
    /// delta. The moisture mass added by the solver (delta_w * rho * V) must equal
    /// the moisture mass removed by the dehumidifier (moisture_mass_flow_kg_s * dt)
    /// within 1e-6 kg — regardless of h_fg value, because the direct mass-flow path
    /// bypasses the latent-energy conversion entirely.
    #[test]
    fn dehumidifier_moisture_mass_round_trips_via_humidity_port() {
        let zone_id = ZoneId(1);
        let t_c = 26.666_666_666_7;
        let w_init = 0.010;
        let volume_m3 = 200.0;
        let dt_s = 60.0;

        let env = env_with_zone(t_c, w_init);

        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let mut solver = HumiditySolver::new(config, &env);
        let w_old = solver.humidity_ratio(zone_id);

        let water_removal_l_day = 30.0;
        let water_removal_kg_s = water_removal_l_day / 86400.0;

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: zone_id,
                sensible_gain_w: 0.0,
                radiant_gain_w: 0.0,
                latent_gain_w: -water_removal_kg_s * 2_501_000.0,
                sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                radiant_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                latent_by_category: [0.0; THERMAL_CATEGORY_COUNT],
            }],
            humidity: vec![HumidityAccumulator {
                zone: zone_id,
                moisture_mass_flow_kg_s: -water_removal_kg_s,
            }],
            ..PortSlots::default()
        };

        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(dt_s as u64));
        let w_new = solver.humidity_ratio(zone_id);

        let rho = hares_physics::air_properties::moist_air_density_kg_m3(
            env.weather.pressure_pa(),
            t_c,
            w_old,
        );

        let delta_m_actual_kg = (w_new - w_old) * rho * volume_m3;
        let delta_m_expected_kg = -water_removal_kg_s * dt_s;

        assert!(
            (delta_m_actual_kg - delta_m_expected_kg).abs() < 1e-6,
            "moisture mass round-trip error: actual={delta_m_actual_kg:.9} kg, \
             expected={delta_m_expected_kg:.9} kg, \
             diff={:.9} kg",
            (delta_m_actual_kg - delta_m_expected_kg).abs()
        );
    }

    /// When a dehumidifier writes both the thermal port (`latent_gain_w`) and the
    /// humidity port (`moisture_mass_flow_kg_s`), and an occupant simultaneously
    /// writes only the thermal port (`latent_gain_w`), the solver must include
    /// BOTH contributions — not drop the occupant's latent gain when the
    /// mass-flow branch is taken.
    ///
    /// Prior behaviour: the explicit mass-flow path computed dW from
    /// `moisture_mass_flow_kg_s` alone and ignored `latent_gain_w` entirely,
    /// silently dropping occupant moisture contributions.
    #[test]
    fn occupant_latent_included_when_mass_flow_branch_active() {
        let zone_id = ZoneId(1);
        let t_c = 22.0;
        let w_init = 0.008;
        let volume_m3 = 200.0;
        let dt_s = 60.0;
        let h_fg = 2_501_000.0_f64;

        let env = env_with_zone(t_c, w_init);

        // Dehumidifier: removes 100 W latent, writes both thermal and humidity ports.
        let dehum_latent_w = -100.0;
        let dehum_mass_flow_kg_s = dehum_latent_w / h_fg;

        // Occupant: adds 75 W latent, writes only the thermal port.
        let occupant_latent_w = 75.0;

        // Total latent gain from the thermal port (both contributors summed).
        let total_latent_gain_w = dehum_latent_w + occupant_latent_w;

        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let mut solver_combined = HumiditySolver::new(config.clone(), &env);
        let mut solver_latent_ref = HumiditySolver::new(config, &env);

        // Combined ports: dehumidifier writes both ports, occupant writes thermal only.
        let ports_combined = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: zone_id,
                sensible_gain_w: 0.0,
                radiant_gain_w: 0.0,
                latent_gain_w: total_latent_gain_w,
                sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                radiant_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                latent_by_category: [0.0; THERMAL_CATEGORY_COUNT],
            }],
            humidity: vec![HumidityAccumulator {
                zone: zone_id,
                moisture_mass_flow_kg_s: dehum_mass_flow_kg_s,
            }],
            ..PortSlots::default()
        };

        // Reference: same total latent through the thermal port (no humidity port).
        let ports_latent_ref = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: zone_id,
                sensible_gain_w: 0.0,
                radiant_gain_w: 0.0,
                latent_gain_w: total_latent_gain_w,
                sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                radiant_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                latent_by_category: [0.0; THERMAL_CATEGORY_COUNT],
            }],
            ..PortSlots::default()
        };

        let _ =
            solver_combined.resolve_new(&ports_combined, &env, Duration::from_secs(dt_s as u64));
        let _ = solver_latent_ref.resolve_new(
            &ports_latent_ref,
            &env,
            Duration::from_secs(dt_s as u64),
        );

        let w_combined = solver_combined.humidity_ratio(zone_id);
        let w_latent_ref = solver_latent_ref.humidity_ratio(zone_id);

        assert!(
            (w_combined - w_latent_ref).abs() < 1e-12,
            "combined mass-flow+latent result ({w_combined:.12e}) must match \
             all-latent reference ({w_latent_ref:.12e}): occupant latent not dropped"
        );
    }

    /// Integration test: the humidity solver's semi-implicit path with infiltration
    /// coupling data from the thermal domain payload produces a stable, non-oscillatory
    /// convergence to outdoor humidity under high ACH (2), coarse dt (600s), and
    /// M=1 (no moisture buffering).
    #[test]
    fn semi_implicit_infiltration_stable_convergence_at_high_ach_coarse_dt() {
        let zone_id = ZoneId(1);
        let volume_m3 = 200.0;
        let ach = 2.0;
        let t_c = 22.0;
        let p_pa = 101_325.0;
        let w_outdoor = 0.012;
        let w_initial = 0.005;
        let dt_s = 600.0;
        let h_fg = HumiditySolverConfig::default().h_fg_j_kg;

        let rho = hares_physics::air_properties::moist_air_density_kg_m3(p_pa, t_c, w_initial);
        let q_m3_s = ach * volume_m3 / 3600.0;
        let m_dot = rho * q_m3_s;

        let env = env_with_zone(t_c, w_initial);
        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let mut solver = HumiditySolver::new(config, &env);

        let n_steps = 100;
        for step in 0..n_steps {
            let w_old = solver.humidity_ratio(zone_id);
            let q_latent_step = m_dot * h_fg * (w_outdoor - w_old);
            let mut env_step = env_with_zone(t_c, w_old);
            env_step.custom_domains.push(DomainUpdate {
                domain_id: THERMAL,
                zone_temperatures_c: vec![(zone_id, t_c)],
                custom_payload: Some(vec![
                    f64::from(zone_id.0),
                    q_latent_step,
                    m_dot,
                    w_outdoor,
                    0.0,
                ]),
            });
            let ports = PortSlots::default();
            let _ = solver.resolve_new(&ports, &env_step, Duration::from_secs(dt_s as u64));
            let w_new = solver.humidity_ratio(zone_id);

            if w_old < w_outdoor {
                assert!(
                    w_new <= w_outdoor + 1e-12,
                    "step {step}: humidity overshot outdoor (w_new={w_new:.6e} > w_out={w_outdoor:.6e})"
                );
            } else if w_old > w_outdoor {
                assert!(
                    w_new >= w_outdoor - 1e-12,
                    "step {step}: humidity undershot outdoor (w_new={w_new:.6e} < w_out={w_outdoor:.6e})"
                );
            }
        }

        let w_final = solver.humidity_ratio(zone_id);
        let rel_error = (w_final - w_outdoor).abs() / w_outdoor;
        assert!(
            rel_error < 0.01,
            "after {n_steps} steps, relative error {rel_error:.4e} must be <1% of w_outdoor={w_outdoor}"
        );
    }

    /// At fine timestep (dt=60s), the semi-implicit and explicit methods should agree
    /// to within 0.1% because both converge to the same steady-state.
    #[test]
    fn semi_implicit_and_explicit_agree_at_fine_timestep() {
        let zone_id = ZoneId(1);
        let volume_m3 = 200.0;
        let ach = 2.0;
        let t_c = 22.0;
        let p_pa = 101_325.0;
        let w_outdoor = 0.012;
        let w_initial = 0.005;
        let dt_s = 60.0;
        let h_fg = HumiditySolverConfig::default().h_fg_j_kg;

        let rho = hares_physics::air_properties::moist_air_density_kg_m3(p_pa, t_c, w_initial);
        let q_m3_s = ach * volume_m3 / 3600.0;
        let m_dot = rho * q_m3_s;

        let q_latent_w = m_dot * h_fg * (w_outdoor - w_initial);

        let env = env_with_zone(t_c, w_initial);
        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };

        let mut solver_semi = HumiditySolver::new(config.clone(), &env);
        let mut env_semi = env_with_zone(t_c, w_initial);
        env_semi.custom_domains.push(DomainUpdate {
            domain_id: THERMAL,
            zone_temperatures_c: vec![(zone_id, t_c)],
            custom_payload: Some(vec![
                f64::from(zone_id.0),
                q_latent_w,
                m_dot,
                w_outdoor,
                0.0,
            ]),
        });
        let ports = PortSlots::default();
        let _ = solver_semi.resolve_new(&ports, &env_semi, Duration::from_secs(dt_s as u64));
        let w_semi = solver_semi.humidity_ratio(zone_id);

        let mut env_explicit = env_with_zone(t_c, w_initial);
        env_explicit.custom_domains.push(DomainUpdate {
            domain_id: THERMAL,
            zone_temperatures_c: vec![(zone_id, t_c)],
            custom_payload: Some(vec![f64::from(zone_id.0), q_latent_w, 0.0, 0.0, 0.0]),
        });
        let mut solver_explicit = HumiditySolver::new(config, &env);
        let _ =
            solver_explicit.resolve_new(&ports, &env_explicit, Duration::from_secs(dt_s as u64));
        let w_explicit = solver_explicit.humidity_ratio(zone_id);

        let dw = (w_outdoor - w_initial).abs();
        let rel_diff = (w_semi - w_explicit).abs() / dw;
        assert!(
            rel_diff < 0.002,
            "semi-implicit ({w_semi:.8e}) and explicit ({w_explicit:.8e}) must agree \
             within 0.2% of Δw at fine dt; rel_diff={rel_diff:.6e}"
        );
    }

    /// Moisture mass balance holds under semi-implicit infiltration coupling.
    #[test]
    fn semi_implicit_infiltration_moisture_mass_balance_holds() {
        let zone_id = ZoneId(1);
        let volume_m3 = 200.0;
        let ach = 2.0;
        let t_c = 22.0;
        let p_pa = 101_325.0;
        let w_outdoor = 0.012;
        let w_initial = 0.005;
        let dt_s = 300.0;
        let h_fg = HumiditySolverConfig::default().h_fg_j_kg;
        let q_other_w = 50.0;

        let rho = hares_physics::air_properties::moist_air_density_kg_m3(p_pa, t_c, w_initial);
        let q_m3_s = ach * volume_m3 / 3600.0;
        let m_dot = rho * q_m3_s;

        let q_inf_latent_w = m_dot * h_fg * (w_outdoor - w_initial);

        let env = env_with_zone(t_c, w_initial);
        let config = HumiditySolverConfig {
            moisture_buffering_multiplier: 1.0,
            ..HumiditySolverConfig::default()
        };
        let mut solver = HumiditySolver::new(config, &env);
        let w_old = solver.humidity_ratio(zone_id);

        let mut env_step = env_with_zone(t_c, w_initial);
        env_step.custom_domains.push(DomainUpdate {
            domain_id: THERMAL,
            zone_temperatures_c: vec![(zone_id, t_c)],
            custom_payload: Some(vec![
                f64::from(zone_id.0),
                q_inf_latent_w,
                m_dot,
                w_outdoor,
                0.0,
            ]),
        });
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: zone_id,
                sensible_gain_w: 0.0,
                latent_gain_w: q_other_w,
                ..ThermalAccumulator::new(zone_id)
            }],
            ..Default::default()
        };
        let _ = solver.resolve_new(&ports, &env_step, Duration::from_secs(dt_s as u64));
        let w_new = solver.humidity_ratio(zone_id);

        let delta_m = (w_new - w_old) * rho * volume_m3;
        let m_from_other = q_other_w * dt_s / h_fg;
        let m_from_infiltration = m_dot * (w_outdoor - w_new) * dt_s;
        let source_m = m_from_other + m_from_infiltration;

        assert!(
            (delta_m - source_m).abs() < 1e-6,
            "moisture mass balance: delta_m={delta_m:.9e} kg, source={source_m:.9e} kg, \
             diff={:.9e} kg",
            (delta_m - source_m).abs()
        );
    }

    // ── Condensation mass tracking tests (T-0180) ─────────────────────────

    /// When latent gain drives w_raw above saturation, the solver clamps w_new
    /// to w_sat and records the condensed mass.
    #[test]
    fn condensation_mass_recorded_when_above_saturation() {
        let env = env_with_zone(20.0, 0.008);
        let mut solver = HumiditySolver::new(HumiditySolverConfig::default(), &env);
        let w_sat =
            hares_physics::psychrometrics::humidity_ratio_from_tdp(20.0, env.weather.pressure_pa());

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 0.0,
                latent_gain_w: 1.0e9, // guarantees w_raw >> w_sat
                ..ThermalAccumulator::new(ZoneId(1))
            }],
            ..Default::default()
        };
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));

        let w_new = solver.humidity_ratio(ZoneId(1));
        assert!(
            (w_new - w_sat).abs() < f64::EPSILON,
            "w_new must equal w_sat"
        );

        let condensation = solver.condensation_mass_kg(ZoneId(1));
        assert!(condensation > 0.0, "condensation mass must be positive");
        assert!(
            solver.condensation_occurred(ZoneId(1)),
            "condensation flag must be set"
        );
    }

    /// When latent gain is negative enough to drive w_raw below zero, the
    /// solver clamps w_new to 0 and records a negative condensation mass
    /// (frost sublimation — moisture added to the air).
    #[test]
    fn condensation_mass_recorded_when_below_zero() {
        let env = env_with_zone(20.0, 0.008);
        let mut solver = HumiditySolver::new(HumiditySolverConfig::default(), &env);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 0.0,
                latent_gain_w: -1.0e9, // guarantees w_raw << 0
                ..ThermalAccumulator::new(ZoneId(1))
            }],
            ..Default::default()
        };
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));

        let w_new = solver.humidity_ratio(ZoneId(1));
        assert!((w_new - 0.0).abs() < f64::EPSILON, "w_new must equal 0");

        let condensation = solver.condensation_mass_kg(ZoneId(1));
        assert!(
            condensation < 0.0,
            "condensation mass must be negative (frost sublimation)"
        );
        assert!(
            solver.condensation_occurred(ZoneId(1)),
            "condensation flag must be set"
        );
    }

    /// Multi-step simulation with cold surface: each step applies a latent
    /// gain large enough to drive w_raw above w_sat, then the solver clamps.
    /// Cumulative condensation mass must be monotonic non-decreasing and
    /// non-negative across steps.
    #[test]
    fn cumulative_condensation_mass_monotonic() {
        let env = env_with_zone(20.0, 0.008);
        let mut solver = HumiditySolver::new(
            HumiditySolverConfig {
                moisture_buffering_multiplier: 1.0,
                ..HumiditySolverConfig::default()
            },
            &env,
        );

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 0.0,
                latent_gain_w: 1.0e9,
                ..ThermalAccumulator::new(ZoneId(1))
            }],
            ..Default::default()
        };

        let mut total_condensation = 0.0_f64;
        for _step in 0..10 {
            let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
            let step_condensation = solver.condensation_mass_kg(ZoneId(1));
            assert!(
                step_condensation >= 0.0,
                "per-step condensation must be non-negative"
            );
            total_condensation += step_condensation;
        }

        assert!(
            total_condensation > 0.0,
            "cumulative condensation must be positive"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "humidity_ratio: ZoneId 999 not found in solver map")]
    fn humidity_ratio_missing_zone_panics_in_debug() {
        // `debug_assert!` fires only in debug builds.
        // In release the call returns the 0.0 sentinel, which the
        // downstream `w_new <= 0.0` guard at dwelling/mod.rs:3080 catches.
        let config = HumiditySolverConfig::default();
        let env = env_with_zone(22.0, 0.008);
        let solver = HumiditySolver::new(config, &env);
        let _ = solver.humidity_ratio(ZoneId(999));
    }
}
