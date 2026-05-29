//! Fluid domain solver for minimal v1 loop energy balance.

use std::collections::HashMap;
use std::time::Duration;

use hares_physics::constants::CP_LIQUID_WATER_J_KG_K;
use hares_types::{
    DomainId, DomainSolver, DomainUpdate, FLUID, FluidDomainPayload, FluidLoopState, FluidType,
    HaresError, LoopId, PortSlots,
};

const MIN_FLOW_KG_S: f64 = 1e-12;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidSolverConfig {
    pub cp_water_j_kg_k: f64,
}

impl Default for FluidSolverConfig {
    fn default() -> Self {
        Self {
            // ASHRAE HoF 2021 Ch.1: 4.18 kJ/(kg·K) ≈ 4180 J/(kg·K).
            // Must match CP_LIQUID_WATER_J_KG_K from hares-physics so that
            // equipment supply temperature calculations (using the same Cp)
            // produce flow-implied energy that matches declared thermal_power_w.
            cp_water_j_kg_k: CP_LIQUID_WATER_J_KG_K,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FluidSolver {
    config: FluidSolverConfig,
    loop_types: HashMap<LoopId, FluidType>,
    loop_states: HashMap<LoopId, FluidLoopState>,
    /// Last known (supply_temp_c, return_temp_c) per loop; used when flow is zero.
    last_known_temps: HashMap<LoopId, (f64, f64)>,
}

impl FluidSolver {
    pub fn new(
        config: FluidSolverConfig,
        declared_loops: &[(LoopId, FluidType)],
    ) -> Result<Self, HaresError> {
        let mut loop_types: HashMap<LoopId, FluidType> = HashMap::new();
        for &(loop_id, fluid_type) in declared_loops {
            if let Some(existing) = loop_types.insert(loop_id, fluid_type)
                && existing != fluid_type
            {
                return Err(HaresError::Envelope(format!(
                    "loop {loop_id:?} declared with conflicting fluid types: {existing:?} and {fluid_type:?}"
                )));
            }
        }
        Ok(Self {
            config,
            loop_types,
            loop_states: HashMap::new(),
            last_known_temps: HashMap::new(),
        })
    }

    #[must_use]
    pub fn loop_state(&self, loop_id: LoopId) -> Option<&FluidLoopState> {
        self.loop_states.get(&loop_id)
    }

    /// Serializes current solver state into a flat `Vec<f64>` for checkpointing.
    ///
    /// Layout: for each entry in `last_known_temps` (sorted by `LoopId`):
    ///   `[loop_id.0 as f64, supply_temp_c, return_temp_c]`
    #[must_use]
    pub fn snapshot_payload(&self) -> Vec<f64> {
        let mut sorted: Vec<_> = self.last_known_temps.iter().collect();
        sorted.sort_by_key(|(id, _)| id.0);
        let mut payload = Vec::with_capacity(sorted.len() * 3);
        for (loop_id, (supply, ret)) in &sorted {
            payload.push(f64::from(loop_id.0));
            payload.push(*supply);
            payload.push(*ret);
        }
        payload
    }

    /// Restores solver state from a checkpoint payload produced by [`snapshot_payload`].
    pub fn restore_from_payload(&mut self, payload: &[f64]) -> Result<(), HaresError> {
        self.last_known_temps.clear();
        if payload.is_empty() {
            return Ok(());
        }
        if !payload.len().is_multiple_of(3) {
            return Err(HaresError::Envelope(format!(
                "fluid checkpoint payload length {} is not a multiple of 3",
                payload.len()
            )));
        }
        for chunk in payload.chunks_exact(3) {
            let loop_id_raw = chunk[0];
            if !loop_id_raw.is_finite() || loop_id_raw < 0.0 || loop_id_raw > f64::from(u16::MAX) {
                return Err(HaresError::Envelope(format!(
                    "invalid loop_id in fluid checkpoint: {loop_id_raw}"
                )));
            }
            let loop_id = LoopId(loop_id_raw as u16);
            self.last_known_temps.insert(loop_id, (chunk[1], chunk[2]));
        }
        Ok(())
    }
}

impl DomainSolver for FluidSolver {
    fn domain_id(&self) -> DomainId {
        FLUID
    }

    fn resolve(
        &mut self,
        ports: &PortSlots,
        _env: &hares_types::EnvironmentState,
        _dt: Duration,
        out: &mut DomainUpdate,
    ) {
        self.loop_states.clear();

        let mut grouped: HashMap<LoopId, Vec<&hares_types::FluidAccumulator>> = HashMap::new();
        for acc in &ports.fluid {
            grouped.entry(acc.loop_id).or_default().push(acc);
        }

        for (loop_id, entries) in grouped {
            let fluid_type = self
                .loop_types
                .get(&loop_id)
                .copied()
                .unwrap_or(entries[0].fluid_type);

            let total_flow: f64 = entries.iter().map(|e| e.total_flow_kg_s).sum();
            let net_power_w: f64 = entries
                .iter()
                .map(|e| {
                    self.config.cp_water_j_kg_k
                        * e.total_flow_kg_s
                        * (e.mean_supply_temp_c - e.mean_return_temp_c)
                })
                .sum();
            let total_declared_thermal_w: f64 =
                entries.iter().map(|e| e.total_thermal_power_w).sum();

            // System-level invariant: when equipment declares thermal power via
            // PortContribution::Fluid.thermal_power_w, the total must match the
            // loop's flow-implied energy balance (T-0084). A mismatch means
            // equipment is declaring energy that the fluid solver cannot confirm
            // from flow × Cp × ΔT — an energy routing gap.
            //
            // Tolerance is 1e-9 × max(|net_power_w|, |declared|, 1.0) —
            // essentially machine epsilon for double-precision. This catches any
            // real mismatch (e.g. equipment writing a dynamic thermal_power_w
            // alongside static config temperatures) while tolerating only
            // floating-point drift in the summation of Cp × flow × ΔT across
            // multiple contributors. A tolerance this tight means the invariant
            // is a hard equality check: the two quantities must come from the
            // same computation path to pass.
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            if total_declared_thermal_w > 0.0 {
                let tol = 1e-9_f64
                    * net_power_w
                        .abs()
                        .max(total_declared_thermal_w.abs())
                        .max(1.0);
                debug_assert!(
                    (net_power_w - total_declared_thermal_w).abs() <= tol,
                    "fluid loop {loop_id:?}: declared thermal power ({total_declared_thermal_w} W) \
                     does not match flow-implied energy balance ({net_power_w} W); \
                     diff = {} W, tol = {tol:e} W",
                    (net_power_w - total_declared_thermal_w).abs()
                );
            }

            let (mean_supply_temp_c, mean_return_temp_c) = if total_flow.abs() <= MIN_FLOW_KG_S {
                self.last_known_temps
                    .get(&loop_id)
                    .copied()
                    .unwrap_or((0.0, 0.0))
            } else {
                let sum_supply = entries
                    .iter()
                    .map(|e| e.total_flow_kg_s * e.mean_supply_temp_c)
                    .sum::<f64>();
                let sum_return = entries
                    .iter()
                    .map(|e| e.total_flow_kg_s * e.mean_return_temp_c)
                    .sum::<f64>();
                let temps = (sum_supply / total_flow, sum_return / total_flow);
                self.last_known_temps.insert(loop_id, temps);
                temps
            };

            self.loop_states.insert(
                loop_id,
                FluidLoopState {
                    loop_id,
                    fluid_type,
                    net_power_w,
                    mean_supply_temp_c,
                    mean_return_temp_c,
                },
            );
        }

        let mut states: Vec<FluidLoopState> = self.loop_states.values().cloned().collect();
        states.sort_by_key(|s| s.loop_id.0);
        out.domain_id = FLUID;
        out.zone_temperatures_c.clear();
        out.custom_payload = FluidDomainPayload::encode(&states);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{FixedOffset, TimeZone};
    use hares_physics::constants::CP_LIQUID_WATER_J_KG_K;
    use hares_types::{
        DomainSolver, EnvironmentState, FluidAccumulator, FluidDomainPayload, FluidType, GridState,
        LoopId, PortContribution, PortSlots, SurfaceIrradiance, WeatherState, ZoneId, ZoneState,
    };

    use crate::fluid_solver::{FluidSolver, FluidSolverConfig};

    fn env() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                outdoor_wet_bulb_c: 7.0,
                outdoor_enthalpy_j_kg: 22_800.0,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
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

    fn approx_eq(a: f64, b: f64) {
        assert!((a - b).abs() <= 1e-6, "a={a} b={b}");
    }

    #[test]
    fn single_loop_net_power_matches_reference() {
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();
        let mut ports = PortSlots {
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.5,
                supply_temp_c: 60.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
            })
            .unwrap();

        let update = solver.resolve_new(&ports, &env(), Duration::from_secs(60));
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        approx_eq(states[0].net_power_w, 0.5 * CP_LIQUID_WATER_J_KG_K * 20.0);
    }

    #[test]
    fn multi_contributor_grouping_is_correct() {
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();
        let mut ports = PortSlots {
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 60.0,
                return_temp_c: 50.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 2.0,
                supply_temp_c: 50.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
            })
            .unwrap();

        let update = solver.resolve_new(&ports, &env(), Duration::from_secs(60));
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        let s = &states[0];
        approx_eq(
            s.net_power_w,
            CP_LIQUID_WATER_J_KG_K * (1.0 * 10.0 + 2.0 * 10.0),
        );
        approx_eq(s.mean_supply_temp_c, (1.0 * 60.0 + 2.0 * 50.0) / 3.0);
        approx_eq(s.mean_return_temp_c, (1.0 * 50.0 + 2.0 * 40.0) / 3.0);
    }

    #[test]
    fn zero_flow_uses_previous_temperatures() {
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();
        let mut ports = PortSlots {
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 52.0,
                return_temp_c: 45.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env(), Duration::from_secs(60));

        let zero_ports = PortSlots {
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..Default::default()
        };
        let update = solver.resolve_new(&zero_ports, &env(), Duration::from_secs(60));
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        approx_eq(states[0].mean_supply_temp_c, 52.0);
        approx_eq(states[0].mean_return_temp_c, 45.0);
    }

    #[test]
    fn empty_ports_returns_none_payload_and_no_state() {
        let mut solver = FluidSolver::new(FluidSolverConfig::default(), &[]).unwrap();
        let ports = PortSlots::default();
        let update = solver.resolve_new(&ports, &env(), Duration::from_secs(60));
        assert_eq!(update.custom_payload, None);
        assert!(solver.loop_state(LoopId(1)).is_none());
    }

    #[test]
    fn loop_state_returns_none_after_no_contributions() {
        // After a step with contributions, if next step has none,
        // loop_state should return None for that loop
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();

        // Step 1: contribute flow to loop 1
        let mut ports = PortSlots {
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..Default::default()
        };
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 55.0,
                return_temp_c: 45.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env(), Duration::from_secs(60));
        assert!(
            solver.loop_state(LoopId(1)).is_some(),
            "loop should have state after contributions"
        );

        // Step 2: no contributions at all (empty fluid vec)
        let empty_ports = PortSlots::default();
        let _ = solver.resolve_new(&empty_ports, &env(), Duration::from_secs(60));
        assert!(
            solver.loop_state(LoopId(1)).is_none(),
            "loop should have no state after step with no contributions"
        );
    }

    #[test]
    fn constructor_rejects_conflicting_fluid_types_for_same_loop() {
        let result = FluidSolver::new(
            FluidSolverConfig::default(),
            &[
                (LoopId(1), FluidType::Water),
                (LoopId(1), FluidType::Glycol),
            ],
        );
        assert!(result.is_err());
    }

    // =======================================================================
    // T-0084: System-level invariant — declared thermal_power_w matches flow balance
    // =======================================================================

    #[test]
    fn declared_thermal_power_w_matches_flow_energy_balance() {
        // When equipment declares thermal_power_w that matches flow × Cp × ΔT,
        // the fluid solver invariant should be satisfied (no panic).
        let mut solver = FluidSolver::new(
            FluidSolverConfig::default(),
            &[(LoopId(1), FluidType::Water)],
        )
        .unwrap();
        let mut ports = PortSlots {
            fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Water)],
            ..Default::default()
        };
        // 0.5 kg/s × Cp J/(kg·K) × 20 K
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.5,
                supply_temp_c: 60.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: Some(0.5 * CP_LIQUID_WATER_J_KG_K * 20.0),
            })
            .unwrap();

        let update = solver.resolve_new(&ports, &env(), Duration::from_secs(60));
        let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
        let declared = 0.5 * CP_LIQUID_WATER_J_KG_K * 20.0;
        let diff = (states[0].net_power_w - declared).abs();
        assert!(
            diff < 1e-3,
            "net_power_w ({}) should match declared thermal_power_w ({declared}); diff = {diff}",
            states[0].net_power_w
        );
    }

    #[test]
    fn thermal_power_w_accumulator_reflects_mixed_contributions() {
        // When one contributor declares thermal_power_w and another does not (None),
        // total_thermal_power_w should equal only the declared contribution.
        let mut ports = PortSlots {
            fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Water)],
            ..Default::default()
        };
        // Contributor A: declares thermal_power_w matching its flow energy
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.3,
                supply_temp_c: 55.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: Some(0.3 * CP_LIQUID_WATER_J_KG_K * 15.0),
            })
            .unwrap();
        // Contributor B: no thermal_power_w declaration (None)
        ports
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 0.2,
                supply_temp_c: 55.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
            })
            .unwrap();

        // total_thermal_power_w should be from contributor A only.
        let expected_declared = 0.3 * CP_LIQUID_WATER_J_KG_K * 15.0;
        let diff = (ports.fluid[0].total_thermal_power_w - expected_declared).abs();
        assert!(
            diff < 1e-3,
            "total_thermal_power_w ({}) should match only declared contribution ({expected_declared})",
            ports.fluid[0].total_thermal_power_w
        );
    }

    #[test]
    fn thermal_power_w_persists_through_zero() {
        // After accumulating, zeroing the accumulator must clear thermal_power_w.
        let mut fluid = FluidAccumulator::new(LoopId(1), FluidType::Water);
        fluid
            .add(0.5, 60.0, 40.0, Some(CP_LIQUID_WATER_J_KG_K))
            .unwrap();
        assert!(fluid.total_thermal_power_w > 0.0);
        fluid.zero();
        assert!((fluid.total_thermal_power_w - 0.0).abs() < 1e-9);
        assert!((fluid.total_flow_kg_s - 0.0).abs() < 1e-9);
    }
}
