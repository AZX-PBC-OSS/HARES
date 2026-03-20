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
pub(crate) fn capture_environment(env: &EnvironmentState) -> EnvironmentCapture {
    EnvironmentCapture {
        outdoor_temp_c: env.weather.outdoor_temp_c,
        ghi_w_m2: env.weather.ghi_w_m2,
        wind_speed_m_s: env.weather.wind_speed_m_s,
        mains_temp_c: env.weather.mains_temp_c,
        zone_temps_c: env.zones.iter().map(|z| (z.id, z.temperature_c)).collect(),
        zone_humidity_ratios: env.zones.iter().map(|z| (z.id, z.humidity_ratio)).collect(),
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
        end_use: desc.end_use,
        telemetry: eq.telemetry().clone(),
        port_declarations: eq.ports().to_vec(),
        contribution,
        pre_step_ports,
    }
}

/// Computes the per-equipment contribution by diffing port accumulators before and after a step.
pub(crate) fn diff_ports(before: &PortSlots, after: &PortSlots) -> EquipmentContribution {
    let thermal: Vec<(ZoneId, f64, f64)> = after
        .thermal
        .iter()
        .zip(before.thermal.iter())
        .map(|(a, b)| {
            (
                a.zone,
                a.sensible_gain_w - b.sensible_gain_w,
                a.latent_gain_w - b.latent_gain_w,
            )
        })
        .filter(|(_, s, l)| s.abs() > f64::EPSILON || l.abs() > f64::EPSILON)
        .collect();

    let fuel_types = [
        FuelType::Electric,
        FuelType::Gas,
        FuelType::Propane,
        FuelType::Oil,
    ];
    let fuel_consumption_w: Vec<(FuelType, f64)> = fuel_types
        .iter()
        .map(|&ft| (ft, after.fuel.get(ft) - before.fuel.get(ft)))
        .filter(|(_, v)| v.abs() > f64::EPSILON)
        .collect();

    const MIN_FLOW_KG_S: f64 = 1e-9;
    let fluid: Vec<FluidContributionCapture> = after
        .fluid
        .iter()
        .zip(before.fluid.iter())
        .filter_map(|(a, b)| {
            let delta_flow = a.total_flow_kg_s - b.total_flow_kg_s;
            if delta_flow.abs() <= MIN_FLOW_KG_S {
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
        electrical_load_kw: after.electrical.load_power_kw - before.electrical.load_power_kw,
        electrical_gen_kw: after.electrical.generation_power_kw
            - before.electrical.generation_power_kw,
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

    let fuel_types = [
        FuelType::Electric,
        FuelType::Gas,
        FuelType::Propane,
        FuelType::Oil,
    ];
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
        })
        .collect();

    PortsCapture {
        thermal,
        electrical_load_kw: ports.electrical.load_power_kw,
        electrical_gen_kw: ports.electrical.generation_power_kw,
        electrical_reactive_kvar: ports.electrical.reactive_power_kvar,
        fuel_consumption_w,
        fluid,
    }
}

/// Captures all four domain solver outputs + envelope component gains.
pub(crate) fn capture_solvers(
    thermal_update: &DomainUpdate,
    humidity_update: &DomainUpdate,
    electrical_update: &DomainUpdate,
    fluid_update: &DomainUpdate,
    thermal_solver: &ThermalSolver,
) -> SolverCapture {
    SolverCapture {
        thermal_update: thermal_update.clone(),
        humidity_update: humidity_update.clone(),
        electrical_update: electrical_update.clone(),
        fluid_update: fluid_update.clone(),
        envelope_gains: thermal_solver.component_gains().clone(),
    }
}

/// Captures final zone temperatures and humidity ratios after zone updates.
pub(crate) fn capture_zone_update(env: &EnvironmentState) -> ZoneUpdateCapture {
    ZoneUpdateCapture {
        zone_temps_c: env.zones.iter().map(|z| (z.id, z.temperature_c)).collect(),
        zone_humidity_ratios: env.zones.iter().map(|z| (z.id, z.humidity_ratio)).collect(),
    }
}
