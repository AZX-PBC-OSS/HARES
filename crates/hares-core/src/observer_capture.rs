//! Pure capture functions that read existing struct fields into observation snapshots.
//!
//! These functions are the only coupling point between Dwelling internals and
//! the observer types. No changes to Equipment, Solver, or any other trait.

use hares_envelope::ThermalSolver;
use hares_equipment::Equipment;
#[cfg(test)]
use hares_types::FluidNodeId;
use hares_types::{
    DomainSolver, DomainUpdate, EnvironmentState, FuelType, PortSlots, ZoneHeat, ZoneId,
};

use crate::observer::{
    CustomSolverCapture, CustomSolverObservation, EnvironmentCapture, EquipmentContribution,
    EquipmentObservation, FluidContributionCapture, FluidPortCapture, PortsCapture, SolverCapture,
    ZoneUpdateCapture,
};

/// Captures weather + zone state from the environment.
pub(crate) fn capture_environment(
    env: &EnvironmentState,
    wf_allows_leap_years: bool,
    raw_mains_temp_f: f64,
    raw_day_of_year: u32,
    civil_day_of_year: u32,
    utc_day_of_year: u32,
) -> EnvironmentCapture {
    let mut capture = EnvironmentCapture {
        outdoor_temp_c: 0.0,
        ghi_w_m2: 0.0,
        dni_w_m2: 0.0,
        dhi_w_m2: 0.0,
        solar_altitude_deg: 0.0,
        solar_azimuth_deg: 0.0,
        wind_speed_m_s: 0.0,
        mains_temp_c: 0.0,
        raw_water_mains_temp_f: 0.0,
        clamped_water_mains_temp_f: 0.0,
        ground_temp_c: 0.0,
        sky_temp_c: 0.0,
        zone_temps_c: Vec::new(),
        zone_humidity_ratios: Vec::new(),
        solar_irradiance: Vec::new(),
        wf_allows_leap_years: false,
        pv_generation_w: 0.0,
        pv_actual_w: 0.0,
        net_grid_w: 0.0,
        base_load_w: 0.0,
        equipment_core_size: 0,
        equipment_telemetry_size: 0,
        raw_day_of_year: 0,
        civil_day_of_year: 0,
        utc_day_of_year: 0,
    };
    capture_environment_into(
        &mut capture,
        env,
        wf_allows_leap_years,
        raw_mains_temp_f,
        raw_day_of_year,
        civil_day_of_year,
        utc_day_of_year,
    );
    capture
}

/// The same capture, written into a recycled capture: the three Vecs
/// (the zone temperatures, the humidity ratios, the solar irradiance)
/// are cleared and refilled through `Vec::clone_from`'s capacity
/// reuse, so the steady state overwrites them in place and allocates
/// nothing. Every field is written: a stale value cannot survive the
/// fill.
pub(crate) fn capture_environment_into(
    capture: &mut EnvironmentCapture,
    env: &EnvironmentState,
    wf_allows_leap_years: bool,
    raw_mains_temp_f: f64,
    raw_day_of_year: u32,
    civil_day_of_year: u32,
    utc_day_of_year: u32,
) {
    use hares_physics::units::temperature_c_to_f;
    let clamped_mains_f = temperature_c_to_f(env.weather.mains_temp_c);
    capture.outdoor_temp_c = env.weather.outdoor_temp_c;
    capture.ghi_w_m2 = env.weather.ghi_w_m2;
    capture.dni_w_m2 = env.weather.dni_w_m2;
    capture.dhi_w_m2 = env.weather.dhi_w_m2;
    capture.solar_altitude_deg = env.weather.solar_altitude_deg;
    capture.solar_azimuth_deg = env.weather.solar_azimuth_deg;
    capture.wind_speed_m_s = env.weather.wind_speed_m_s;
    capture.mains_temp_c = env.weather.mains_temp_c;
    capture.raw_water_mains_temp_f = raw_mains_temp_f;
    capture.clamped_water_mains_temp_f = clamped_mains_f;
    capture.ground_temp_c = env.weather.ground_temp_c;
    capture.sky_temp_c = env.weather.sky_temp_c;
    capture.zone_temps_c.clear();
    capture
        .zone_temps_c
        .extend(env.zones.iter().map(|z| (z.id, z.temperature_c)));
    capture.zone_humidity_ratios.clear();
    capture
        .zone_humidity_ratios
        .extend(env.zones.iter().map(|z| (z.id, z.humidity_ratio)));
    capture
        .solar_irradiance
        .clone_from(&env.weather.solar_irradiance);
    capture.wf_allows_leap_years = wf_allows_leap_years;
    capture.pv_generation_w = env.electrical.pv_generation_w;
    capture.pv_actual_w = env.electrical.actual_pv_w;
    capture.net_grid_w = env.electrical.net_grid_w;
    capture.base_load_w = env.electrical.base_load_w;
    capture.equipment_core_size = env.equipment_core.len();
    capture.equipment_telemetry_size = env.equipment_telemetry.len();
    capture.raw_day_of_year = raw_day_of_year;
    capture.civil_day_of_year = civil_day_of_year;
    capture.utc_day_of_year = utc_day_of_year;
}

/// Captures a single equipment's observation with contribution and pre-step port state.
///
/// The fresh form: an empty observation written through
/// [`capture_single_equipment_into`], so the two forms' field set is
/// mirrored in one place.
pub(crate) fn capture_single_equipment(
    eq: &dyn Equipment,
    contribution: EquipmentContribution,
    pre_step_ports: PortsCapture,
) -> EquipmentObservation {
    let mut obs = EquipmentObservation::default();
    capture_single_equipment_into(&mut obs, eq, contribution, Some(pre_step_ports));
    obs
}

/// The same capture, written into a recycled observation: the telemetry
/// map's copy goes through `Telemetry`'s hand-written `clone_from`, so
/// the steady state (every key registered at init, the same key set on
/// both sides) overwrites the values in place and allocates nothing.
/// The identity fields still build fresh Strings: their
/// borrowed-or-shared descriptor form is the equipment capture's own
/// remaining site, not this one.
///
/// `pre_step_ports` is `Some` for a fresh observation (the capture
/// moves in) and `None` for a recycled one whose `pre_step_ports` was
/// already written in place before the equipment stepped (the
/// pre-step capture must precede the step, so the recycled path writes
/// it through [`capture_ports_into`] at the pre-step site).
pub(crate) fn capture_single_equipment_into(
    obs: &mut EquipmentObservation,
    eq: &dyn Equipment,
    contribution: EquipmentContribution,
    pre_step_ports: Option<PortsCapture>,
) {
    let desc = eq.descriptor();
    obs.name = desc.name.clone();
    obs.equipment_type = desc.equipment_type.to_string();
    obs.end_use = desc.end_use.clone();
    #[cfg(test)]
    let before = hares_types::alloc_count::thread_allocations();
    obs.telemetry.clone_from(eq.telemetry());
    #[cfg(test)]
    record_telemetry_site_allocations(before);
    obs.port_declarations = eq.ports().to_vec();
    obs.contribution = contribution;
    if let Some(pre_step_ports) = pre_step_ports {
        obs.pre_step_ports = pre_step_ports;
    }
}

// The observation path's telemetry-write site's allocation bracket: the
// count the counting allocator saw around the site's map copy, read and
// reset by the dwelling's witness tests. This file is the only place the
// site itself sits.
#[cfg(test)]
thread_local! {
    static TELEMETRY_SITE_ALLOCATIONS: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_telemetry_site_allocations(before: Option<u64>) {
    if let (Some(before), Some(after)) = (before, hares_types::alloc_count::thread_allocations()) {
        TELEMETRY_SITE_ALLOCATIONS.with(|cell| {
            cell.set(cell.get() + after.saturating_sub(before));
        });
    }
}

/// Reads and resets the telemetry-write site's allocation bracket.
#[cfg(test)]
pub(crate) fn take_telemetry_site_allocations() -> u64 {
    TELEMETRY_SITE_ALLOCATIONS.with(std::cell::Cell::take)
}

/// Computes the per-equipment contribution by diffing port accumulators before and after a step.
pub(crate) fn diff_ports(before: &PortSlots, after: &PortSlots) -> EquipmentContribution {
    // Match by ZoneId rather than positional index for robustness.
    let thermal: Vec<(ZoneId, ZoneHeat)> = after
        .thermal
        .iter()
        .filter_map(|a| {
            let b = before.thermal.iter().find(|b| b.zone == a.zone)?;
            let added = a.heat().since(&b.heat());
            added.exceeds(f64::EPSILON).then_some((a.zone, added))
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
                direction: a.direction,
            })
        })
        .collect();

    EquipmentContribution {
        thermal,
        electrical_load_w: after.electrical.load_power_w - before.electrical.load_power_w,
        electrical_gen_w: after.electrical.generation_power_w
            - before.electrical.generation_power_w,
        electrical_reactive_var: after.electrical.reactive_power_var
            - before.electrical.reactive_power_var,
        fuel_consumption_w,
        fluid,
    }
}

/// Captures a snapshot of all port accumulators.
pub(crate) fn capture_ports(ports: &PortSlots) -> PortsCapture {
    let mut capture = PortsCapture::default();
    capture_ports_into(&mut capture, ports);
    capture
}

/// The same capture, written into a recycled capture: the three Vecs
/// are cleared and refilled in place, so the steady state reuses their
/// capacities and allocates nothing. Every field is written: a stale
/// value cannot survive the fill.
pub(crate) fn capture_ports_into(capture: &mut PortsCapture, ports: &PortSlots) {
    capture.thermal.clear();
    capture
        .thermal
        .extend(ports.thermal.iter().map(|t| (t.zone, t.heat())));

    let fuel_types = hares_types::ports::ALL_FUEL_TYPES;

    // Compile-time: ALL_FUEL_TYPES must carry every real fuel type.
    const _: () = assert!(
        hares_types::ports::ALL_FUEL_TYPES.len() == hares_types::ports::FUEL_TYPE_COUNT,
        "ALL_FUEL_TYPES length must match FUEL_TYPE_COUNT; ensure new FuelType variants are added to ALL_FUEL_TYPES"
    );

    capture.fuel_consumption_w.clear();
    capture
        .fuel_consumption_w
        .extend(fuel_types.iter().map(|&ft| (ft, ports.fuel.get(ft))));

    capture.fluid.clear();
    capture
        .fluid
        .extend(ports.fluid.iter().map(|f| FluidPortCapture {
            loop_id: f.loop_id,
            fluid_type: f.fluid_type,
            total_flow_kg_s: f.total_flow_kg_s,
            mean_supply_temp_c: f.mean_supply_temp_c,
            mean_return_temp_c: f.mean_return_temp_c,
            total_thermal_power_w: f.total_thermal_power_w,
            direction: f.direction,
        }));

    capture.electrical_load_w = ports.electrical.load_power_w;
    capture.electrical_gen_w = ports.electrical.generation_power_w;
    capture.electrical_reactive_var = ports.electrical.reactive_power_var;
}

/// Electrical port balance observability scalars for [`capture_solvers`],
/// grouped because they describe one ZIP/port balance state.
pub(crate) struct PortBalance {
    pub zip_load_scale: f64,
    pub port_load_raw_w: f64,
    pub port_load_adjusted_w: f64,
    pub residual_w: f64,
}

/// Captures all four domain solver outputs + envelope component gains,
/// plus electrical balance observability for ZIP-adjusted invariant checking.
pub(crate) fn capture_solvers(
    thermal_update: &DomainUpdate,
    humidity_update: &DomainUpdate,
    electrical_update: &DomainUpdate,
    fluid_update: &DomainUpdate,
    thermal_solver: &ThermalSolver,
    balance: PortBalance,
) -> SolverCapture {
    let mut capture = SolverCapture {
        thermal_update: thermal_update.clone(),
        humidity_update: humidity_update.clone(),
        electrical_update: electrical_update.clone(),
        fluid_update: fluid_update.clone(),
        envelope_gains: thermal_solver.component_gains().clone(),
        zip_load_scale: balance.zip_load_scale,
        port_load_raw_w: balance.port_load_raw_w,
        port_load_adjusted_w: balance.port_load_adjusted_w,
        residual_w: balance.residual_w,
    };
    capture_solvers_into(
        &mut capture,
        thermal_update,
        humidity_update,
        electrical_update,
        fluid_update,
        thermal_solver,
        balance,
    );
    capture
}

/// The same capture, written into a recycled capture: the four domain
/// updates go through `DomainUpdate`'s in-place `clone_from` and the
/// envelope gains through `EnvelopeComponentGains::copy_in_place`, so
/// the steady state overwrites them in place and allocates nothing.
/// Every field is written: a stale value cannot survive the fill.
pub(crate) fn capture_solvers_into(
    capture: &mut SolverCapture,
    thermal_update: &DomainUpdate,
    humidity_update: &DomainUpdate,
    electrical_update: &DomainUpdate,
    fluid_update: &DomainUpdate,
    thermal_solver: &ThermalSolver,
    balance: PortBalance,
) {
    let PortBalance {
        zip_load_scale,
        port_load_raw_w,
        port_load_adjusted_w,
        residual_w,
    } = balance;
    capture.thermal_update.clone_from(thermal_update);
    capture.humidity_update.clone_from(humidity_update);
    capture.electrical_update.clone_from(electrical_update);
    capture.fluid_update.clone_from(fluid_update);
    capture
        .envelope_gains
        .clone_from(thermal_solver.component_gains());
    capture.zip_load_scale = zip_load_scale;
    capture.port_load_raw_w = port_load_raw_w;
    capture.port_load_adjusted_w = port_load_adjusted_w;
    capture.residual_w = residual_w;
}

/// Captures final zone temperatures and humidity ratios after zone updates.
pub(crate) fn capture_zone_update(env: &EnvironmentState) -> ZoneUpdateCapture {
    let mut capture = ZoneUpdateCapture {
        zone_temps_c: Vec::new(),
        zone_humidity_ratios: Vec::new(),
    };
    capture_zone_update_into(&mut capture, env);
    capture
}

/// The same capture, written into a recycled capture: the two Vecs are
/// cleared and refilled in place, so the steady state reuses their
/// capacities and allocates nothing.
pub(crate) fn capture_zone_update_into(capture: &mut ZoneUpdateCapture, env: &EnvironmentState) {
    capture.zone_temps_c.clear();
    capture
        .zone_temps_c
        .extend(env.zones.iter().map(|z| (z.id, z.temperature_c)));
    capture.zone_humidity_ratios.clear();
    capture
        .zone_humidity_ratios
        .extend(env.zones.iter().map(|z| (z.id, z.humidity_ratio)));
}

/// Captures observation state from all custom domain solvers after their
/// resolve step.
pub(crate) fn capture_custom_solvers(solvers: &[Box<dyn DomainSolver>]) -> CustomSolverCapture {
    CustomSolverCapture {
        solvers: solvers
            .iter()
            .map(|s| CustomSolverObservation {
                domain_id: s.domain_id(),
                state: s.observation_state(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hares_types::{
        ElectricalAccumulator, FluidAccumulator, FluidType, FuelAccumulator, FuelType, GridState,
        HaresError, HeatTransferDirection, LoopId, PortSlots, SurfaceIrradiance,
        ThermalAccumulator, WeatherState, ZoneHeat, ZoneId, ZoneState,
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
        approx_eq(contrib.thermal[0].1.convective_w, 250.0);
        assert_eq!(contrib.thermal[1].0, zone_b);
        approx_eq(contrib.thermal[1].1.convective_w, -50.0);
        approx_eq(contrib.thermal[1].1.latent_w, 5.0);
    }

    /// A contribution that is only radiant or only short-wave is captured,
    /// and the captured heat of every zone equals its port total.
    #[test]
    fn observer_sees_radiant_and_shortwave_heat() {
        let zone = ZoneId(1);
        let before = PortSlots {
            thermal: vec![ThermalAccumulator::new(zone)],
            ..Default::default()
        };
        let after = PortSlots {
            thermal: vec![ThermalAccumulator {
                radiant_gain_w: 300.0,
                shortwave_gain_w: 100.0,
                ..ThermalAccumulator::new(zone)
            }],
            ..Default::default()
        };

        let contrib = diff_ports(&before, &after);
        assert_eq!(contrib.thermal.len(), 1, "a radiant-only gain is captured");
        let added = contrib.thermal[0].1;
        approx_eq(added.radiant_w, 300.0);
        approx_eq(added.shortwave_w, 100.0);

        let captured = capture_ports(&after);
        for ((zone, heat), port) in captured.thermal.iter().zip(&after.thermal) {
            assert_eq!(*zone, port.zone);
            approx_eq(heat.total_w(), port.total_gain_w());
        }
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
            electrical: ElectricalAccumulator {
                load_power_w: 1.0,
                generation_power_w: -2.0,
                reactive_power_var: 0.5,
                ..Default::default()
            },
            ..Default::default()
        };
        let after = PortSlots {
            electrical: ElectricalAccumulator {
                load_power_w: 4.0,
                generation_power_w: -2.0,
                reactive_power_var: 1.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let contrib = diff_ports(&before, &after);
        approx_eq(contrib.electrical_load_w, 3.0);
        approx_eq(contrib.electrical_gen_w, 0.0);
        approx_eq(contrib.electrical_reactive_var, 0.5);
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
                node_id: FluidNodeId(0),
                total_flow_kg_s: 1.0,
                mean_supply_temp_c: 40.0,
                mean_return_temp_c: 30.0,
                total_thermal_power_w: 0.0,
                direction: Some(HeatTransferDirection::Source),
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
                node_id: FluidNodeId(0),
                total_flow_kg_s: 3.0,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 35.0,
                total_thermal_power_w: 0.0,
                direction: Some(HeatTransferDirection::Source),
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
                node_id: FluidNodeId(0),
                total_flow_kg_s: 10.0,
                mean_supply_temp_c: 50.0,
                mean_return_temp_c: 40.0,
                total_thermal_power_w: 0.0,
                direction: Some(HeatTransferDirection::Source),
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

    /// The in-place capture's overwrite-completeness pin: the destination
    /// is made dirty field by field (independent of the fill function, so
    /// an omitted write cannot hide behind a capture the same function
    /// built), then refilled from a different source. The result must
    /// equal the fresh form on every field: a stale scalar, a stale zone
    /// or fuel row, or a stale fluid row means the in-place form skipped
    /// a field. The literal is exhaustive, so a new field breaks it.
    #[test]
    fn capture_ports_into_overwrites_a_dirty_capture() {
        let mut clean_source = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(2),
                sensible_gain_w: 7.0,
                ..ThermalAccumulator::new(ZoneId(2))
            }],
            ..Default::default()
        };
        clean_source
            .fuel
            .add(FuelType::Wood, 42.0)
            .expect("wood registers");
        let fresh = capture_ports(&clean_source);

        let dirty = PortsCapture {
            thermal: vec![
                (
                    ZoneId(1),
                    ZoneHeat {
                        convective_w: 1.0,
                        radiant_w: 2.0,
                        shortwave_w: 3.0,
                        latent_w: 4.0,
                    },
                ),
                (
                    ZoneId(2),
                    ZoneHeat {
                        convective_w: 5.0,
                        radiant_w: 6.0,
                        shortwave_w: 7.0,
                        latent_w: 8.0,
                    },
                ),
            ],
            electrical_load_w: 101.0,
            electrical_gen_w: 102.0,
            electrical_reactive_var: 103.0,
            fuel_consumption_w: hares_types::ALL_FUEL_TYPES
                .iter()
                .map(|&fuel_type| (fuel_type, 200.0))
                .collect(),
            fluid: vec![FluidPortCapture {
                loop_id: LoopId(3),
                fluid_type: FluidType::Water,
                total_flow_kg_s: 0.5,
                mean_supply_temp_c: 40.0,
                mean_return_temp_c: 30.0,
                total_thermal_power_w: 1000.0,
                direction: Some(HeatTransferDirection::Source),
            }],
        };

        let mut recycled = dirty;
        capture_ports_into(&mut recycled, &clean_source);

        assert_eq!(
            format!("{recycled:?}"),
            format!("{fresh:?}"),
            "the recycled capture's refill must equal the fresh capture \
             on every field: a stale zone row, fuel entry, fluid row or \
             scalar means the in-place form skipped a field"
        );
    }

    /// An environment state whose every captured scalar is derived from
    /// `base`, with one zone per entry and `surfaces` irradiance rows:
    /// two states with different bases differ in every field the
    /// environment capture reads, so a skipped in-place write cannot
    /// read as a match.
    fn environment_with_scalars(
        base: f64,
        zones: &[(u16, f64, f64)],
        surfaces: u32,
    ) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: zones
                .iter()
                .map(|&(id, temperature_c, humidity_ratio)| {
                    ZoneState::new(ZoneId(id), temperature_c, humidity_ratio, 120.0)
                })
                .collect(),
            weather: WeatherState {
                outdoor_temp_c: base + 1.0,
                ghi_w_m2: base + 2.0,
                dni_w_m2: base + 3.0,
                dhi_w_m2: base + 4.0,
                solar_altitude_deg: base + 5.0,
                solar_azimuth_deg: base + 6.0,
                wind_speed_m_s: base + 7.0,
                mains_temp_c: base + 8.0,
                ground_temp_c: base + 9.0,
                sky_temp_c: base + 10.0,
                solar_irradiance: (0..surfaces)
                    .map(|surface_id| SurfaceIrradiance {
                        surface_id,
                        direct_w_m2: base + f64::from(surface_id) + 11.0,
                        diffuse_w_m2: base + 12.0,
                        reflected_w_m2: base + 13.0,
                        angle_of_incidence_rad: 0.5,
                    })
                    .collect(),
                ..WeatherState::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
            equipment_telemetry: zones
                .iter()
                .map(|&(id, _, _)| (format!("equipment-{id}"), hares_types::Telemetry::default()))
                .collect(),
            equipment_core: Default::default(),
            current_time: chrono::DateTime::parse_from_rfc3339("2024-06-15T12:00:00Z")
                .expect("a valid timestamp"),
            time_res: chrono::Duration::minutes(1),
            price_signal: hares_types::PriceSignal::default(),
            electrical: hares_types::ElectricalSummary {
                pv_generation_w: base + 14.0,
                actual_pv_w: base + 15.0,
                net_grid_w: base + 16.0,
                base_load_w: base + 17.0,
                ..hares_types::ElectricalSummary::default()
            },
        }
    }

    /// The environment and zone-update captures' overwrite-completeness
    /// pin: each destination is made dirty field by field (independent of
    /// the fill function) and refilled from a different source; the
    /// result must equal the fresh form on every field. A stale zone row,
    /// irradiance row or scalar means the in-place form skipped a field.
    /// Both literals are exhaustive, so a new field breaks them.
    #[test]
    fn environment_and_zone_update_into_overwrite_a_dirty_capture() {
        let clean_source = environment_with_scalars(200.0, &[(2, 18.0, 0.006)], 1);

        let mut recycled_environment = EnvironmentCapture {
            outdoor_temp_c: 901.0,
            ghi_w_m2: 902.0,
            dni_w_m2: 903.0,
            dhi_w_m2: 904.0,
            solar_altitude_deg: 905.0,
            solar_azimuth_deg: 906.0,
            wind_speed_m_s: 907.0,
            mains_temp_c: 908.0,
            raw_water_mains_temp_f: 909.0,
            clamped_water_mains_temp_f: 910.0,
            ground_temp_c: 911.0,
            sky_temp_c: 912.0,
            zone_temps_c: vec![(ZoneId(1), 21.0), (ZoneId(2), 22.0), (ZoneId(3), 23.0)],
            zone_humidity_ratios: vec![(ZoneId(1), 0.1), (ZoneId(2), 0.2), (ZoneId(3), 0.3)],
            solar_irradiance: vec![
                SurfaceIrradiance {
                    surface_id: 0,
                    direct_w_m2: 1.0,
                    diffuse_w_m2: 2.0,
                    reflected_w_m2: 3.0,
                    angle_of_incidence_rad: 0.1,
                },
                SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 4.0,
                    diffuse_w_m2: 5.0,
                    reflected_w_m2: 6.0,
                    angle_of_incidence_rad: 0.2,
                },
            ],
            wf_allows_leap_years: false,
            pv_generation_w: 913.0,
            pv_actual_w: 914.0,
            net_grid_w: 915.0,
            base_load_w: 916.0,
            equipment_core_size: 7,
            equipment_telemetry_size: 8,
            raw_day_of_year: 9,
            civil_day_of_year: 10,
            utc_day_of_year: 11,
        };
        capture_environment_into(
            &mut recycled_environment,
            &clean_source,
            true,
            41.0,
            12,
            13,
            14,
        );
        let fresh_environment = capture_environment(&clean_source, true, 41.0, 12, 13, 14);
        assert_eq!(
            format!("{recycled_environment:?}"),
            format!("{fresh_environment:?}"),
            "the recycled environment capture's refill must equal the fresh \
             capture on every field: a stale zone row, irradiance row or \
             scalar means the in-place form skipped a field"
        );

        let mut recycled_zone_update = ZoneUpdateCapture {
            zone_temps_c: vec![(ZoneId(1), 90.0), (ZoneId(2), 91.0), (ZoneId(3), 92.0)],
            zone_humidity_ratios: vec![(ZoneId(1), 0.9), (ZoneId(2), 0.8), (ZoneId(3), 0.7)],
        };
        capture_zone_update_into(&mut recycled_zone_update, &clean_source);
        let fresh_zone_update = capture_zone_update(&clean_source);
        assert_eq!(
            format!("{recycled_zone_update:?}"),
            format!("{fresh_zone_update:?}"),
            "the recycled zone-update capture's refill must equal the fresh \
             capture on every field: a stale zone row means the in-place \
             form skipped a field"
        );
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

    #[test]
    fn equipment_phase_capture_includes_wood_fuel() {
        let mut fuel = FuelAccumulator::default();
        fuel.add(FuelType::Wood, 2000.0).unwrap();
        fuel.add(FuelType::Coal, 500.0).unwrap();
        fuel.add(FuelType::WoodPellet, 750.0).unwrap();
        fuel.add(FuelType::Gas, 100.0).unwrap();

        let ports = PortSlots {
            fuel,
            ..Default::default()
        };

        let mut phase_ports = PortsCapture::default();
        capture_ports_into(&mut phase_ports, &ports);

        let find = |ft: FuelType| -> f64 {
            phase_ports
                .fuel_consumption_w
                .iter()
                .find(|(t, _)| *t == ft)
                .map_or(0.0, |(_, v)| *v)
        };

        approx_eq(find(FuelType::Wood), 2000.0);
        approx_eq(find(FuelType::Coal), 500.0);
        approx_eq(find(FuelType::WoodPellet), 750.0);
        approx_eq(find(FuelType::Gas), 100.0);
    }

    #[test]
    fn capture_custom_solvers_records_domain_id_and_state() {
        use hares_types::{DomainId, DomainSolver, EnvironmentState, PortSlots};
        use std::time::Duration;

        struct StubSolver {
            id: DomainId,
            state: Vec<f64>,
        }

        impl DomainSolver for StubSolver {
            fn domain_id(&self) -> DomainId {
                self.id
            }
            fn resolve(
                &mut self,
                _ports: &PortSlots,
                _env: &EnvironmentState,
                _dt: Duration,
                _out: &mut DomainUpdate,
            ) -> std::result::Result<(), HaresError> {
                Ok(())
            }
            fn observation_state(&self) -> Vec<f64> {
                self.state.clone()
            }
        }

        let s1: Box<dyn DomainSolver> = Box::new(StubSolver {
            id: DomainId(42),
            state: vec![1.0, 2.0, 3.0],
        });
        let s2: Box<dyn DomainSolver> = Box::new(StubSolver {
            id: DomainId(99),
            state: vec![],
        });
        let solvers: Vec<Box<dyn DomainSolver>> = vec![s1, s2];

        let capture = capture_custom_solvers(&solvers);

        assert_eq!(capture.solvers.len(), 2);
        assert_eq!(capture.solvers[0].domain_id, DomainId(42));
        assert_eq!(capture.solvers[0].state, vec![1.0, 2.0, 3.0]);
        assert_eq!(capture.solvers[1].domain_id, DomainId(99));
        assert!(capture.solvers[1].state.is_empty());
    }

    #[test]
    fn diff_ports_reactive_emitting_equipment_shows_q_in_contribution() {
        let before = PortSlots {
            electrical: ElectricalAccumulator {
                load_power_w: 5000.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let after = PortSlots {
            electrical: ElectricalAccumulator {
                load_power_w: 5000.0,
                reactive_power_var: 3.23,
                ..Default::default()
            },
            ..Default::default()
        };
        let contrib = diff_ports(&before, &after);
        approx_eq(contrib.electrical_load_w, 0.0);
        approx_eq(contrib.electrical_gen_w, 0.0);
        approx_eq(contrib.electrical_reactive_var, 3.23);
    }

    #[test]
    fn diff_ports_reactive_supplying_equipment_shows_negative_q() {
        let before = PortSlots {
            electrical: ElectricalAccumulator {
                generation_power_w: -2000.0,
                reactive_power_var: 0.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let after = PortSlots {
            electrical: ElectricalAccumulator {
                generation_power_w: -2000.0,
                reactive_power_var: -1.5,
                ..Default::default()
            },
            ..Default::default()
        };
        let contrib = diff_ports(&before, &after);
        approx_eq(contrib.electrical_reactive_var, -1.5);
    }

    #[test]
    fn capture_custom_solvers_empty_input_yields_empty_capture() {
        let solvers: Vec<Box<dyn DomainSolver>> = vec![];
        let capture = capture_custom_solvers(&solvers);
        assert!(capture.solvers.is_empty());
    }
}
