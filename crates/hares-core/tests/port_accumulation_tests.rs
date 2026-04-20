//! Integration tests for PortSlots multi-equipment accumulation patterns.
//!
//! These tests exercise `PortSlots` from `hares-core`'s perspective: the
//! declaration → accumulate → read → zero lifecycle as it occurs across an
//! entire simulated timestep with multiple equipment contributions.

use hares_types::{
    DomainId, FluidType, FuelType, LoopId, PortContribution, PortDeclaration, PortSlots,
    ThermalAccumulator, ThermalCategory, ZoneId,
};

fn approx_eq(left: f64, right: f64) {
    assert!(
        (left - right).abs() < 1e-9,
        "expected {right}, got {left} (diff {})",
        (left - right).abs()
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `PortSlots` wired for the given zones plus an electrical and fuel
/// singleton -- mirrors what `Dwelling::from_preparsed` does for each zone.
fn slots_for_zones(zones: &[ZoneId]) -> PortSlots {
    let mut decls: Vec<PortDeclaration> =
        zones.iter().map(|&z| PortDeclaration::thermal(z)).collect();
    decls.push(PortDeclaration::electrical());
    decls.push(PortDeclaration::fuel());
    PortSlots::from_declarations(&decls)
}

fn thermal(zone: ZoneId, sensible_w: f64, latent_w: f64) -> PortContribution {
    PortContribution::Thermal {
        zone,
        sensible_gain_w: sensible_w,
        latent_gain_w: latent_w,
        category: ThermalCategory::InternalGain,
    }
}

fn electrical(active_kw: f64) -> PortContribution {
    PortContribution::Electrical {
        active_power_kw: active_kw,
        reactive_power_kvar: 0.0,
    }
}

fn gas(consumption_w: f64) -> PortContribution {
    PortContribution::Fuel {
        fuel_type: FuelType::Gas,
        consumption_w,
    }
}

// ---------------------------------------------------------------------------
// Electrical accumulation
// ---------------------------------------------------------------------------

#[test]
fn electrical_accumulates_across_equipment() {
    let mut ports = PortSlots::default();
    for _ in 0..3 {
        ports.accumulate(&electrical(1.0)).unwrap();
    }
    approx_eq(ports.electrical.load_power_kw, 3.0);
    approx_eq(ports.electrical.net_active_kw(), 3.0);
    approx_eq(ports.electrical.generation_power_kw, 0.0);
}

#[test]
fn generation_subtracts_from_net() {
    let mut ports = PortSlots::default();
    ports.accumulate(&electrical(3.0)).unwrap(); // 3 kW load
    ports.accumulate(&electrical(-5.0)).unwrap(); // 5 kW PV
    approx_eq(ports.electrical.net_active_kw(), -2.0);
    approx_eq(ports.electrical.load_power_kw, 3.0);
    approx_eq(ports.electrical.generation_power_kw, -5.0);
}

#[test]
fn mixed_load_and_generation_multiple_sources() {
    // Two loads (2 kW + 1.5 kW) and two generators (-3 kW + -0.75 kW).
    let mut ports = PortSlots::default();
    ports.accumulate(&electrical(2.0)).unwrap();
    ports.accumulate(&electrical(1.5)).unwrap();
    ports.accumulate(&electrical(-3.0)).unwrap();
    ports.accumulate(&electrical(-0.75)).unwrap();

    approx_eq(ports.electrical.load_power_kw, 3.5);
    approx_eq(ports.electrical.generation_power_kw, -3.75);
    approx_eq(ports.electrical.net_active_kw(), -0.25);
}

#[test]
fn reactive_power_accumulates_algebraically() {
    let mut ports = PortSlots::default();
    ports
        .accumulate(&PortContribution::Electrical {
            active_power_kw: 1.0,
            reactive_power_kvar: 0.5,
        })
        .unwrap();
    ports
        .accumulate(&PortContribution::Electrical {
            active_power_kw: 1.0,
            reactive_power_kvar: -0.2,
        })
        .unwrap();
    approx_eq(ports.electrical.reactive_power_kvar, 0.3);
}

// ---------------------------------------------------------------------------
// Thermal accumulation across zones
// ---------------------------------------------------------------------------

#[test]
fn thermal_accumulates_per_zone() {
    let zone1 = ZoneId(1);
    let zone2 = ZoneId(2);
    let mut ports = slots_for_zones(&[zone1, zone2]);

    // Zone 1: two contributions
    ports.accumulate(&thermal(zone1, 500.0, 50.0)).unwrap();
    ports.accumulate(&thermal(zone1, 200.0, 20.0)).unwrap();
    // Zone 2: one contribution
    ports.accumulate(&thermal(zone2, 300.0, 30.0)).unwrap();

    let z1 = ports
        .thermal
        .iter()
        .find(|t| t.zone == zone1)
        .expect("zone 1 accumulator must exist");
    let z2 = ports
        .thermal
        .iter()
        .find(|t| t.zone == zone2)
        .expect("zone 2 accumulator must exist");

    approx_eq(z1.sensible_gain_w, 700.0);
    approx_eq(z1.latent_gain_w, 70.0);
    approx_eq(z2.sensible_gain_w, 300.0);
    approx_eq(z2.latent_gain_w, 30.0);
}

#[test]
fn thermal_zones_are_independent() {
    // Contributions to zone 1 must not bleed into zone 2 and vice-versa.
    let zone1 = ZoneId(1);
    let zone2 = ZoneId(2);
    let mut ports = slots_for_zones(&[zone1, zone2]);

    ports.accumulate(&thermal(zone1, 1000.0, 0.0)).unwrap();
    ports.accumulate(&thermal(zone2, 0.0, 500.0)).unwrap();

    let z1 = ports.thermal.iter().find(|t| t.zone == zone1).unwrap();
    let z2 = ports.thermal.iter().find(|t| t.zone == zone2).unwrap();

    approx_eq(z1.sensible_gain_w, 1000.0);
    approx_eq(z1.latent_gain_w, 0.0);
    approx_eq(z2.sensible_gain_w, 0.0);
    approx_eq(z2.latent_gain_w, 500.0);
}

// ---------------------------------------------------------------------------
// Fuel accumulation
// ---------------------------------------------------------------------------

#[test]
fn fuel_accumulates_by_type() {
    let mut ports = slots_for_zones(&[ZoneId(1)]);

    // Two gas consumers (furnace + water heater)
    ports.accumulate(&gas(8_000.0)).unwrap(); // furnace: ~8 kW
    ports.accumulate(&gas(4_000.0)).unwrap(); // water heater: ~4 kW

    approx_eq(ports.fuel.get(FuelType::Gas), 12_000.0);
    // Other fuel types unaffected
    approx_eq(ports.fuel.get(FuelType::Electric), 0.0);
    approx_eq(ports.fuel.get(FuelType::Propane), 0.0);
    approx_eq(ports.fuel.get(FuelType::Oil), 0.0);
}

#[test]
fn fuel_types_are_independent() {
    let mut ports = slots_for_zones(&[ZoneId(1)]);
    ports
        .accumulate(&PortContribution::Fuel {
            fuel_type: FuelType::Propane,
            consumption_w: 5_000.0,
        })
        .unwrap();
    ports
        .accumulate(&PortContribution::Fuel {
            fuel_type: FuelType::Oil,
            consumption_w: 2_000.0,
        })
        .unwrap();

    approx_eq(ports.fuel.get(FuelType::Gas), 0.0);
    approx_eq(ports.fuel.get(FuelType::Propane), 5_000.0);
    approx_eq(ports.fuel.get(FuelType::Oil), 2_000.0);
}

// ---------------------------------------------------------------------------
// zero() clears all accumulators
// ---------------------------------------------------------------------------

#[test]
fn zero_after_reset_clears_all_accumulators() {
    let zone = ZoneId(1);
    let mut ports = slots_for_zones(&[zone]);

    // Accumulate into every port type
    ports.accumulate(&thermal(zone, 500.0, 100.0)).unwrap();
    ports.accumulate(&electrical(4.0)).unwrap();
    ports.accumulate(&gas(10_000.0)).unwrap();

    ports.zero();

    let z = ports.thermal.iter().find(|t| t.zone == zone).unwrap();
    approx_eq(z.sensible_gain_w, 0.0);
    approx_eq(z.latent_gain_w, 0.0);
    approx_eq(ports.electrical.net_active_kw(), 0.0);
    approx_eq(ports.electrical.load_power_kw, 0.0);
    approx_eq(ports.electrical.generation_power_kw, 0.0);
    approx_eq(ports.electrical.reactive_power_kvar, 0.0);
    approx_eq(ports.fuel.get(FuelType::Gas), 0.0);
}

#[test]
fn zero_then_accumulate_starts_fresh() {
    // After zero(), a second round of accumulation must not add to the previous values.
    let zone = ZoneId(1);
    let mut ports = slots_for_zones(&[zone]);

    ports.accumulate(&thermal(zone, 999.0, 999.0)).unwrap();
    ports.accumulate(&electrical(9.0)).unwrap();

    ports.zero();

    ports.accumulate(&thermal(zone, 100.0, 10.0)).unwrap();
    ports.accumulate(&electrical(2.0)).unwrap();

    let z = ports.thermal.iter().find(|t| t.zone == zone).unwrap();
    approx_eq(z.sensible_gain_w, 100.0);
    approx_eq(z.latent_gain_w, 10.0);
    approx_eq(ports.electrical.load_power_kw, 2.0);
}

// ---------------------------------------------------------------------------
// from_declarations deduplication
// ---------------------------------------------------------------------------

#[test]
fn from_declarations_deduplicates_zones() {
    let decls = [
        PortDeclaration::thermal(ZoneId(1)),
        PortDeclaration::thermal(ZoneId(1)), // duplicate
        PortDeclaration::thermal(ZoneId(2)),
        PortDeclaration::thermal(ZoneId(2)), // duplicate
    ];
    let ports = PortSlots::from_declarations(&decls);
    assert_eq!(
        ports.thermal.len(),
        2,
        "duplicate zone declarations must be collapsed to one accumulator each"
    );
    assert!(ports.thermal.iter().any(|t| t.zone == ZoneId(1)));
    assert!(ports.thermal.iter().any(|t| t.zone == ZoneId(2)));
}

#[test]
fn from_declarations_deduplicates_fluid_loops() {
    let decls = [
        PortDeclaration::fluid(LoopId(1), FluidType::Water),
        PortDeclaration::fluid(LoopId(1), FluidType::Water), // exact duplicate
        PortDeclaration::fluid(LoopId(1), FluidType::Glycol), // same loop, different fluid: distinct
        PortDeclaration::fluid(LoopId(2), FluidType::Water),
    ];
    let ports = PortSlots::from_declarations(&decls);
    assert_eq!(ports.fluid.len(), 3);
}

#[test]
fn from_declarations_all_thermal_accumulators_start_at_zero() {
    let decls = [
        PortDeclaration::thermal(ZoneId(5)),
        PortDeclaration::thermal(ZoneId(6)),
    ];
    let ports = PortSlots::from_declarations(&decls);
    for acc in &ports.thermal {
        approx_eq(acc.sensible_gain_w, 0.0);
        approx_eq(acc.latent_gain_w, 0.0);
    }
}

// ---------------------------------------------------------------------------
// Undeclared port rejection
// ---------------------------------------------------------------------------

#[test]
fn undeclared_zone_rejected() {
    let mut ports = slots_for_zones(&[ZoneId(1)]);
    let result = ports.accumulate(&thermal(ZoneId(99), 100.0, 0.0));
    assert!(
        result.is_err(),
        "accumulate to an undeclared zone must return Err"
    );
}

#[test]
fn undeclared_fluid_loop_rejected() {
    let decls = [PortDeclaration::fluid(LoopId(1), FluidType::Water)];
    let mut ports = PortSlots::from_declarations(&decls);
    let result = ports.accumulate(&PortContribution::Fluid {
        loop_id: LoopId(99),
        flow_rate_kg_s: 1.0,
        supply_temp_c: 40.0,
        return_temp_c: 35.0,
        fluid_type: FluidType::Water,
    });
    assert!(
        result.is_err(),
        "accumulate to an undeclared fluid loop must return Err"
    );
}

#[test]
fn undeclared_custom_domain_rejected() {
    let decls = [PortDeclaration::custom(DomainId(1))];
    let mut ports = PortSlots::from_declarations(&decls);
    let payload = [0.0f64; 16];
    let result = ports.accumulate(&PortContribution::Custom {
        domain_id: DomainId(99),
        payload,
    });
    assert!(
        result.is_err(),
        "accumulate to an undeclared custom domain must return Err"
    );
}

// ---------------------------------------------------------------------------
// Multi-timestep lifecycle (zero between steps)
// ---------------------------------------------------------------------------

#[test]
fn multi_timestep_accumulation_is_independent() {
    // Simulate two consecutive timesteps: each should start fresh after zero().
    let zone = ZoneId(1);
    let mut ports = slots_for_zones(&[zone]);

    // Timestep 1
    ports.accumulate(&thermal(zone, 300.0, 30.0)).unwrap();
    ports.accumulate(&electrical(2.0)).unwrap();
    ports.accumulate(&gas(5_000.0)).unwrap();

    let t1_sensible = ports.thermal[0].sensible_gain_w;
    let t1_electric = ports.electrical.load_power_kw;
    let t1_gas = ports.fuel.get(FuelType::Gas);

    ports.zero();

    // Timestep 2 with different values
    ports.accumulate(&thermal(zone, 150.0, 15.0)).unwrap();
    ports.accumulate(&electrical(1.0)).unwrap();
    ports.accumulate(&gas(2_500.0)).unwrap();

    approx_eq(t1_sensible, 300.0);
    approx_eq(t1_electric, 2.0);
    approx_eq(t1_gas, 5_000.0);

    approx_eq(ports.thermal[0].sensible_gain_w, 150.0);
    approx_eq(ports.electrical.load_power_kw, 1.0);
    approx_eq(ports.fuel.get(FuelType::Gas), 2_500.0);
}

// ---------------------------------------------------------------------------
// ThermalAccumulator standalone API
// ---------------------------------------------------------------------------

#[test]
fn thermal_accumulator_new_starts_zeroed() {
    let acc = ThermalAccumulator::new(ZoneId(42));
    assert_eq!(acc.zone, ZoneId(42));
    approx_eq(acc.sensible_gain_w, 0.0);
    approx_eq(acc.latent_gain_w, 0.0);
}

#[test]
fn thermal_accumulator_add_is_additive() {
    let mut acc = ThermalAccumulator::new(ZoneId(1));
    acc.add(100.0, 50.0, ThermalCategory::InternalGain);
    acc.add(-30.0, 10.0, ThermalCategory::InternalGain);
    approx_eq(acc.sensible_gain_w, 70.0);
    approx_eq(acc.latent_gain_w, 60.0);
}

#[test]
fn thermal_accumulator_zero_resets() {
    let mut acc = ThermalAccumulator::new(ZoneId(1));
    acc.add(500.0, 200.0, ThermalCategory::HvacHeating);
    acc.zero();
    approx_eq(acc.sensible_gain_w, 0.0);
    approx_eq(acc.latent_gain_w, 0.0);
}

// ---------------------------------------------------------------------------
// Full simulated-timestep scenario
// ---------------------------------------------------------------------------

#[test]
fn full_timestep_scenario_all_port_types() {
    // Simulate one complete equipment pass for a two-zone building with:
    //   - PV array (generation)
    //   - plug loads + lighting (load)
    //   - gas furnace (thermal + gas)
    //   - gas water heater (gas only)
    let zone_main = ZoneId(1);
    let zone_garage = ZoneId(2);

    let mut decls: Vec<PortDeclaration> = vec![
        PortDeclaration::thermal(zone_main),
        PortDeclaration::thermal(zone_garage),
        PortDeclaration::electrical(),
        PortDeclaration::fuel(),
    ];
    decls.push(PortDeclaration::fluid(LoopId(1), FluidType::Water));

    let mut ports = PortSlots::from_declarations(&decls);

    // PV: -4.5 kW
    ports.accumulate(&electrical(-4.5)).unwrap();
    // Plug loads: 1.2 kW
    ports.accumulate(&electrical(1.2)).unwrap();
    // Lighting: 0.3 kW
    ports.accumulate(&electrical(0.3)).unwrap();
    // Gas furnace: heats main zone, consumes gas
    ports.accumulate(&thermal(zone_main, 6_000.0, 0.0)).unwrap();
    ports
        .accumulate(&PortContribution::Fuel {
            fuel_type: FuelType::Gas,
            consumption_w: 9_000.0,
        })
        .unwrap();
    // Gas water heater: gas only
    ports
        .accumulate(&PortContribution::Fuel {
            fuel_type: FuelType::Gas,
            consumption_w: 3_000.0,
        })
        .unwrap();
    // Fluid loop pump
    ports
        .accumulate(&PortContribution::Fluid {
            loop_id: LoopId(1),
            flow_rate_kg_s: 0.2,
            supply_temp_c: 55.0,
            return_temp_c: 45.0,
            fluid_type: FluidType::Water,
        })
        .unwrap();

    // Verify electrical net
    approx_eq(ports.electrical.generation_power_kw, -4.5);
    approx_eq(ports.electrical.load_power_kw, 1.5);
    approx_eq(ports.electrical.net_active_kw(), -3.0);

    // Verify thermal
    let main = ports.thermal.iter().find(|t| t.zone == zone_main).unwrap();
    approx_eq(main.sensible_gain_w, 6_000.0);
    approx_eq(main.latent_gain_w, 0.0);
    let garage = ports
        .thermal
        .iter()
        .find(|t| t.zone == zone_garage)
        .unwrap();
    approx_eq(garage.sensible_gain_w, 0.0);

    // Verify fuel
    approx_eq(ports.fuel.get(FuelType::Gas), 12_000.0);
    approx_eq(ports.fuel.get(FuelType::Electric), 0.0);

    // Verify fluid
    assert_eq!(ports.fluid.len(), 1);
    approx_eq(ports.fluid[0].total_flow_kg_s, 0.2);
    approx_eq(ports.fluid[0].mean_supply_temp_c, 55.0);
    approx_eq(ports.fluid[0].mean_return_temp_c, 45.0);
}
