//! Pure capture functions that read existing struct fields into observation snapshots.
//!
//! These functions are the only coupling point between Dwelling internals and
//! the observer types. No changes to Equipment, Solver, or any other trait.

use hares_envelope::ThermalSolver;
use hares_equipment::Equipment;
use hares_types::{DomainUpdate, EnvironmentState, FuelType, PortSlots, ZoneId};

use crate::observer::{
    EnvironmentCapture, EquipmentContribution, EquipmentObservation, EquipmentPhaseCapture,
    FluidContributionCapture, FluidPortCapture, PortsCapture, SolverCapture, ZoneUpdateCapture,
};

/// Captures weather + zone state from the environment.
pub(crate) fn capture_environment(
    env: &EnvironmentState,
    wf_allows_leap_years: bool,
) -> EnvironmentCapture {
    EnvironmentCapture {
        outdoor_temp_c: env.weather.outdoor_temp_c,
        ghi_w_m2: env.weather.ghi_w_m2,
        dni_w_m2: env.weather.dni_w_m2,
        dhi_w_m2: env.weather.dhi_w_m2,
        solar_altitude_deg: env.weather.solar_altitude_deg,
        solar_azimuth_deg: env.weather.solar_azimuth_deg,
        wind_speed_m_s: env.weather.wind_speed_m_s,
        mains_temp_c: env.weather.mains_temp_c,
        ground_temp_c: env.weather.ground_temp_c,
        sky_temp_c: env.weather.sky_temp_c,
        zone_temps_c: env.zones.iter().map(|z| (z.id, z.temperature_c)).collect(),
        zone_humidity_ratios: env.zones.iter().map(|z| (z.id, z.humidity_ratio)).collect(),
        solar_irradiance: env.weather.solar_irradiance.clone(),
        wf_allows_leap_years,
    }
}

/// Builds an `EquipmentPhaseCapture` from pre-built per-equipment observations.
pub(crate) fn capture_equipment_phase(
    observations: Vec<EquipmentObservation>,
    ports: &PortSlots,
) -> EquipmentPhaseCapture {
    EquipmentPhaseCapture {
        equipment: observations,
        ports: capture_ports(ports),
    }
}

/// Captures a single equipment's observation with contribution and pre-step port state.
pub(crate) fn capture_single_equipment(
    eq: &dyn Equipment,
    contribution: EquipmentContribution,
    pre_step_ports: PortsCapture,
) -> EquipmentObservation {
    let desc = eq.descriptor();
    EquipmentObservation {
        name: desc.name.clone(),
        equipment_type: desc.equipment_type.to_string(),
        end_use: desc.end_use.clone(),
        telemetry: eq.telemetry().clone(),
        port_declarations: eq.ports().to_vec(),
        contribution,
        pre_step_ports,
        zone_id_explicit: eq.zone_id_explicit(),
    }
}

/// Computes the per-equipment contribution by diffing port accumulators before and after a step.
pub(crate) fn diff_ports(before: &PortSlots, after: &PortSlots) -> EquipmentContribution {
    // Match by ZoneId rather than positional index for robustness.
    let thermal: Vec<(ZoneId, f64, f64)> = after
        .thermal
        .iter()
        .filter_map(|a| {
            let b = before.thermal.iter().find(|b| b.zone == a.zone)?;
            let ds = a.sensible_gain_w - b.sensible_gain_w;
            let dl = a.latent_gain_w - b.latent_gain_w;
            (ds.abs() > f64::EPSILON || dl.abs() > f64::EPSILON).then_some((a.zone, ds, dl))
        })
        .collect();

    use hares_types::ports::ALL_FUEL_TYPES;

    let fuel_types = &ALL_FUEL_TYPES;
    let fuel_consumption_w: Vec<(FuelType, f64)> = fuel_types
        .iter()
        .map(|&ft| (ft, after.fuel.get(ft) - before.fuel.get(ft)))
        .filter(|(_, v)| v.abs() > f64::EPSILON)
        .collect();

    // Threshold raised to avoid catastrophic cancellation when back-calculating
    // temperatures from the flow-weighted mean formula with large prior flows.
    const MIN_DELTA_FLOW_KG_S: f64 = 1e-6;
    // Match by (loop_id, fluid_type) rather than positional index.
    let fluid: Vec<FluidContributionCapture> = after
        .fluid
        .iter()
        .filter_map(|a| {
            let b = before
                .fluid
                .iter()
                .find(|b| b.loop_id == a.loop_id && b.fluid_type == a.fluid_type)?;
            let delta_flow = a.total_flow_kg_s - b.total_flow_kg_s;
            if delta_flow.abs() <= MIN_DELTA_FLOW_KG_S {
                return None;
            }
            // Back-calculate this equipment's supply/return temps from flow-weighted means.
            let supply_temp_c = (a.mean_supply_temp_c * a.total_flow_kg_s
                - b.mean_supply_temp_c * b.total_flow_kg_s)
                / delta_flow;
            let return_temp_c = (a.mean_return_temp_c * a.total_flow_kg_s
                - b.mean_return_temp_c * b.total_flow_kg_s)
                / delta_flow;
            Some(FluidContributionCapture {
                loop_id: a.loop_id,
                fluid_type: a.fluid_type,
                delta_flow_kg_s: delta_flow,
                supply_temp_c,
                return_temp_c,
            })
        })
        .collect();

    EquipmentContribution {
        thermal,
        electrical_load_kw: after.electrical.load_power_w - before.electrical.load_power_w,
        electrical_gen_kw: after.electrical.generation_power_w
            - before.electrical.generation_power_w,
        electrical_reactive_kvar: after.electrical.reactive_power_kvar
            - before.electrical.reactive_power_kvar,
        fuel_consumption_w,
        fluid,
    }
}

/// Captures a snapshot of all port accumulators.
pub(crate) fn capture_ports(ports: &PortSlots) -> PortsCapture {
    let thermal: Vec<(ZoneId, f64, f64)> = ports
        .thermal
        .iter()
        .map(|t| (t.zone, t.sensible_gain_w, t.latent_gain_w))
        .collect();

    let fuel_types = hares_types::ports::ALL_FUEL_TYPES;

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    debug_assert_eq!(
        fuel_types.len(),
        hares_types::ports::FUEL_TYPE_COUNT,
        "ALL_FUEL_TYPES length must match FUEL_TYPE_COUNT; ensure new FuelType variants are added to ALL_FUEL_TYPES"
    );

    let fuel_consumption_w: Vec<(FuelType, f64)> = fuel_types
        .iter()
        .map(|&ft| (ft, ports.fuel.get(ft)))
        .collect();

    let fluid: Vec<FluidPortCapture> = ports
        .fluid
        .iter()
        .map(|f| FluidPortCapture {
            loop_id: f.loop_id,
            fluid_type: f.fluid_type,
            total_flow_kg_s: f.total_flow_kg_s,
            mean_supply_temp_c: f.mean_supply_temp_c,
            mean_return_temp_c: f.mean_return_temp_c,
            total_thermal_power_w: f.total_thermal_power_w,
        })
        .collect();

    PortsCapture {
        thermal,
        electrical_load_kw: ports.electrical.load_power_w,
        electrical_gen_kw: ports.electrical.generation_power_w,
        electrical_reactive_kvar: ports.electrical.reactive_power_kvar,
        fuel_consumption_w,
        fluid,
    }
}

/// Captures all four domain solver outputs + envelope component gains,
/// plus electrical balance observability for ZIP-adjusted invariant checking.
pub(crate) fn capture_solvers(
    thermal_update: &DomainUpdate,
    humidity_update: &DomainUpdate,
    electrical_update: &DomainUpdate,
    fluid_update: &DomainUpdate,
    thermal_solver: &ThermalSolver,
    zip_load_scale: f64,
    port_load_raw_kw: f64,
    port_load_adjusted_kw: f64,
    residual_kw: f64,
) -> SolverCapture {
    SolverCapture {
        thermal_update: thermal_update.clone(),
        humidity_update: humidity_update.clone(),
        electrical_update: electrical_update.clone(),
        fluid_update: fluid_update.clone(),
        envelope_gains: thermal_solver.component_gains().clone(),
        zip_load_scale,
        port_load_raw_kw,
        port_load_adjusted_kw,
        residual_kw,
    }
}

/// Captures final zone temperatures and humidity ratios after zone updates.
pub(crate) fn capture_zone_update(env: &EnvironmentState) -> ZoneUpdateCapture {
    ZoneUpdateCapture {
        zone_temps_c: env.zones.iter().map(|z| (z.id, z.temperature_c)).collect(),
        zone_humidity_ratios: env.zones.iter().map(|z| (z.id, z.humidity_ratio)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hares_types::{
        ElectricalAccumulator, FluidAccumulator, FluidType, FuelAccumulator, FuelType, LoopId,
        PortSlots, ThermalAccumulator, ZoneId,
    };

    fn approx_eq(left: f64, right: f64) {
        assert!((left - right).abs() < 1e-9, "left={left}, right={right}");
    }

    #[test]
    fn diff_ports_thermal_contributions() {
        let zone_a = ZoneId(1);
        let zone_b = ZoneId(2);
        let before = PortSlots {
            thermal: vec![
                ThermalAccumulator {
                    zone: zone_a,
                    sensible_gain_w: 100.0,
                    latent_gain_w: 10.0,
                    ..ThermalAccumulator::new(zone_a)
                },
                ThermalAccumulator {
                    zone: zone_b,
                    sensible_gain_w: 0.0,
                    latent_gain_w: 0.0,
                    ..ThermalAccumulator::new(zone_b)
                },
            ],
            ..Default::default()
        };
        let after = PortSlots {
            thermal: vec![
                ThermalAccumulator {
                    zone: zone_a,
                    sensible_gain_w: 350.0,
                    latent_gain_w: 10.0,
                    ..ThermalAccumulator::new(zone_a)
                },
                ThermalAccumulator {
                    zone: zone_b,
                    sensible_gain_w: -50.0,
                    latent_gain_w: 5.0,
                    ..ThermalAccumulator::new(zone_b)
                },
            ],
            ..Default::default()
        };

        let contrib = diff_ports(&before, &after);
        // zone_a: +250 sensible, 0 latent (filtered)
        // zone_b: -50 sensible, +5 latent
        assert_eq!(contrib.thermal.len(), 2);
        assert_eq!(contrib.thermal[0].0, zone_a);
        approx_eq(contrib.thermal[0].1, 250.0);
        assert_eq!(contrib.thermal[1].0, zone_b);
        approx_eq(contrib.thermal[1].1, -50.0);
        approx_eq(contrib.thermal[1].2, 5.0);
    }

    #[test]
    fn diff_ports_zero_delta_thermal_filtered() {
        let zone = ZoneId(1);
        let slots = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone,
                sensible_gain_w: 42.0,
                latent_gain_w: 7.0,
                ..ThermalAccumulator::new(zone)
            }],
            ..Default::default()
        };
        let contrib = diff_ports(&slots, &slots);
        assert!(contrib.thermal.is_empty());
    }

    #[test]
    fn diff_ports_electrical() {
        let before = PortSlots {
            electrical: {
                let mut e = ElectricalAccumulator::default();
                e.load_power_w = 1.0;
                e.generation_power_w = -2.0;
                e.reactive_power_kvar = 0.5;
                e
            },
            ..Default::default()
        };
        let after = PortSlots {
            electrical: {
                let mut e = ElectricalAccumulator::default();
                e.load_power_w = 4.0;
                e.generation_power_w = -2.0;
                e.reactive_power_kvar = 1.0;
                e
            },
            ..Default::default()
        };
        let contrib = diff_ports(&before, &after);
        approx_eq(contrib.electrical_load_kw, 3.0);
        approx_eq(contrib.electrical_gen_kw, 0.0);
        approx_eq(contrib.electrical_reactive_kvar, 0.5);
    }

    #[test]
    fn diff_ports_fuel() {
        let mut before_fuel = FuelAccumulator::default();
        before_fuel.add(FuelType::Gas, 100.0).unwrap();
        let mut after_fuel = FuelAccumulator::default();
        after_fuel.add(FuelType::Gas, 350.0).unwrap();
        after_fuel.add(FuelType::Propane, 50.0).unwrap();

        let before = PortSlots {
            fuel: before_fuel,
            ..Default::default()
        };
        let after = PortSlots {
            fuel: after_fuel,
            ..Default::default()
        };
        let contrib = diff_ports(&before, &after);
        assert_eq!(contrib.fuel_consumption_w.len(), 2);
        let gas = contrib
            .fuel_consumption_w
            .iter()
            .find(|(ft, _)| *ft == FuelType::Gas);
        approx_eq(gas.unwrap().1, 250.0);
        let propane = contrib
            .fuel_consumption_w
            .iter()
            .find(|(ft, _)| *ft == FuelType::Propane);
        approx_eq(propane.unwrap().1, 50.0);
    }

    #[test]
    fn diff_ports_fluid_back_calculates_temps() {
        let loop_id = LoopId(1);
        let ft = FluidType::Water;
        // Before: 1.0 kg/s at supply=40, return=30
        let before = PortSlots {
            fluid: vec![FluidAccumulator {
                loop_id,
                fluid_type: ft,
                total_flow_kg_s: 1.0,
                mean_supply_temp_c: 40.0,
                mean_return_temp_c: 30.0,
                total_thermal_power_w: 0.0,
            }],
            ..Default::default()
        };
        // After: 3.0 kg/s at supply=45, return=35
        // Equipment added 2.0 kg/s. Its supply = (45*3 - 40*1)/2 = 47.5
        // Its return = (35*3 - 30*1)/2 = 37.5
        let after = PortSlots {
            fluid: vec![FluidAccumulator {
                loop_id,
                fluid_type: ft,
                total_flow_kg_s: 3.0,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 35.0,
                total_thermal_power_w: 0.0,
            }],
            ..Default::default()
        };

        let contrib = diff_ports(&before, &after);
        assert_eq!(contrib.fluid.len(), 1);
        approx_eq(contrib.fluid[0].delta_flow_kg_s, 2.0);
        approx_eq(contrib.fluid[0].supply_temp_c, 47.5);
        approx_eq(contrib.fluid[0].return_temp_c, 37.5);
    }

    #[test]
    fn diff_ports_fluid_tiny_delta_filtered() {
        let loop_id = LoopId(1);
        let ft = FluidType::Water;
        let slots = PortSlots {
            fluid: vec![FluidAccumulator {
                loop_id,
                fluid_type: ft,
                total_flow_kg_s: 10.0,
                mean_supply_temp_c: 50.0,
                mean_return_temp_c: 40.0,
                total_thermal_power_w: 0.0,
            }],
            ..Default::default()
        };
        let contrib = diff_ports(&slots, &slots);
        assert!(contrib.fluid.is_empty());
    }

    #[test]
    fn capture_ports_includes_all_fuel_types() {
        let mut fuel = FuelAccumulator::default();
        fuel.add(FuelType::Gas, 1000.0).unwrap();
        fuel.add(FuelType::Wood, 2000.0).unwrap();
        fuel.add(FuelType::Coal, 3000.0).unwrap();
        fuel.add(FuelType::WoodPellet, 4000.0).unwrap();

        let ports = PortSlots {
            fuel,
            ..Default::default()
        };

        let capture = capture_ports(&ports);

        let find = |ft: FuelType| -> f64 {
            capture
                .fuel_consumption_w
                .iter()
                .find(|(t, _)| *t == ft)
                .map_or(0.0, |(_, v)| *v)
        };

        approx_eq(find(FuelType::Gas), 1000.0);
        approx_eq(find(FuelType::Wood), 2000.0);
        approx_eq(find(FuelType::Coal), 3000.0);
        approx_eq(find(FuelType::WoodPellet), 4000.0);
        approx_eq(find(FuelType::Electric), 0.0);
        approx_eq(find(FuelType::Propane), 0.0);
        approx_eq(find(FuelType::Oil), 0.0);
    }

    #[test]
    fn diff_ports_detects_wood_coal_woodpellet() {
        let mut before_fuel = FuelAccumulator::default();
        before_fuel.add(FuelType::Wood, 500.0).unwrap();
        before_fuel.add(FuelType::Coal, 1000.0).unwrap();

        let mut after_fuel = FuelAccumulator::default();
        after_fuel.add(FuelType::Wood, 1500.0).unwrap();
        after_fuel.add(FuelType::Coal, 1000.0).unwrap(); // unchanged
        after_fuel.add(FuelType::WoodPellet, 750.0).unwrap();

        let before = PortSlots {
            fuel: before_fuel,
            ..Default::default()
        };
        let after = PortSlots {
            fuel: after_fuel,
            ..Default::default()
        };
        let contrib = diff_ports(&before, &after);

        // Wood: 1500 - 500 = 1000 (present)
        // Coal: 1000 - 1000 = 0 (filtered)
        // WoodPellet: 750 - 0 = 750 (present)
        assert_eq!(contrib.fuel_consumption_w.len(), 2);

        let wood = contrib
            .fuel_consumption_w
            .iter()
            .find(|(ft, _)| *ft == FuelType::Wood);
        approx_eq(wood.unwrap().1, 1000.0);

        let pellet = contrib
            .fuel_consumption_w
            .iter()
            .find(|(ft, _)| *ft == FuelType::WoodPellet);
        approx_eq(pellet.unwrap().1, 750.0);

        let coal = contrib
            .fuel_consumption_w
            .iter()
            .find(|(ft, _)| *ft == FuelType::Coal);
        assert!(coal.is_none(), "zero delta must be filtered out");
    }
}
