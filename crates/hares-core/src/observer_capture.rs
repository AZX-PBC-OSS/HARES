//! Pure capture functions that read existing struct fields into observation snapshots.
//!
//! These functions are the only coupling point between Dwelling internals and
//! the observer types. No changes to Equipment, Solver, or any other trait.

use hares_envelope::ThermalSolver;
use hares_equipment::Equipment;
use hares_types::{DomainUpdate, EnvironmentState, FuelType, PortSlots, ZoneId};

use crate::observer::{
    EnvironmentCapture, EquipmentObservation, EquipmentPhaseCapture, FluidPortCapture,
    PortsCapture, SolverCapture, ZoneUpdateCapture,
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

/// Captures equipment telemetry and port accumulator state after an equipment phase.
pub(crate) fn capture_equipment_phase(
    equipment: &[Box<dyn Equipment>],
    ports: &PortSlots,
) -> EquipmentPhaseCapture {
    let observations: Vec<EquipmentObservation> = equipment
        .iter()
        .map(|eq| {
            let desc = eq.descriptor();
            EquipmentObservation {
                name: desc.name.clone(),
                equipment_type: desc.equipment_type.to_string(),
                end_use: desc.end_use,
                telemetry: eq.telemetry().clone(),
            }
        })
        .collect();

    EquipmentPhaseCapture {
        equipment: observations,
        ports: capture_ports(ports),
    }
}

/// Captures a snapshot of all port accumulators.
fn capture_ports(ports: &PortSlots) -> PortsCapture {
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
