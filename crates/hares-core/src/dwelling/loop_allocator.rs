//! Centralized fluid loop ID allocator for equipment specs.
//!
//! After `resolve_loop_wiring` assigns IDs to cross-referenced combi pairs,
//! this pass scans all typed configs, finds the maximum already-assigned loop
//! ID, and assigns unique sequential IDs to equipment whose typed config
//! carries a `None` loop_id (or `boiler_loop_id` for indirect tanks).
//!
//! This eliminates the collision risk where standalone equipment constructors
//! hardcoded `LoopId(1)` as a default, potentially colliding with a combi-pair
//! assigned `LoopId(1)` by `resolve_loop_wiring`.

use hares_equipment::{
    ElectricBoilerConfig, ElectricResistanceWaterHeaterConfig, EquipmentConfig,
    EquipmentTypedConfig, GasBoilerConfig, GasWaterHeaterConfig, GeneratorConfig,
    HeatPumpWaterHeaterConfig, IndirectTankConfig, TanklessWaterHeaterConfig,
};
use hares_io::EquipmentSpec;
use hares_types::HaresError;

/// Iterate over all allocated loop IDs across equipment typed configs.
///
/// Yields the `loop_id` (or `boiler_loop_id` for indirect tanks) from every
/// equipment spec whose typed config carries a `Some` value.  This is the
/// single source of truth for which equipment types carry loop IDs — both
/// `max_wired_loop_id` and `collect_allocated_loop_ids` derive from it,
/// guaranteeing that adding a new equipment type to this match arm
/// automatically covers allocation, validation, and max-finding.
fn iter_allocated_loop_ids(specs: &[EquipmentSpec]) -> impl Iterator<Item = u16> + '_ {
    specs.iter().filter_map(|spec| {
        let cfg = spec.typed_config.as_ref()?;
        match spec.name.as_str() {
            "Gas Boiler" => cfg.typed::<GasBoilerConfig>().ok()?.loop_id,
            "Electric Boiler" => cfg.typed::<ElectricBoilerConfig>().ok()?.loop_id,
            "Gas Water Heater" => cfg.typed::<GasWaterHeaterConfig>().ok()?.loop_id,
            "Electric Resistance Water Heater" => {
                cfg.typed::<ElectricResistanceWaterHeaterConfig>()
                    .ok()?
                    .loop_id
            }
            "Tankless Water Heater" => cfg.typed::<TanklessWaterHeaterConfig>().ok()?.loop_id,
            "Heat Pump Water Heater" => cfg.typed::<HeatPumpWaterHeaterConfig>().ok()?.loop_id,
            "Indirect Tank" => cfg.typed::<IndirectTankConfig>().ok()?.boiler_loop_id,
            "Gas Generator" | "Gas Fuel Cell" => cfg.typed::<GeneratorConfig>().ok()?.loop_id,
            _ => None,
        }
    })
}

/// Scan all equipment typed configs and find the maximum assigned loop ID.
///
/// Loop ID 0 (the `Default` for `LoopId`) is never explicitly assigned by
/// `resolve_loop_wiring` (which starts from 1).  Returning 0 when no IDs are
/// wired produces the correct `max_wired + 1 = 1` start for the allocator.
fn max_wired_loop_id(specs: &[EquipmentSpec]) -> u16 {
    iter_allocated_loop_ids(specs).max().unwrap_or(0)
}

/// Patch a typed config in-place when `loop_id` field is `None`.
fn replace_typed<T: EquipmentTypedConfig>(
    cfg: &mut Option<EquipmentConfig>,
    next_id: &mut u16,
    setter: impl FnOnce(&mut T, u16),
    assign: impl FnOnce(&T) -> bool,
) -> Result<(), HaresError> {
    let Some(eq_cfg) = cfg else {
        return Ok(());
    };
    let Ok(mut typed) = eq_cfg.typed::<T>() else {
        return Ok(());
    };
    if !assign(&typed) {
        return Ok(());
    }
    setter(&mut typed, *next_id);
    *next_id = next_id.saturating_add(1);
    *eq_cfg = EquipmentConfig::from_typed(eq_cfg.name.clone(), eq_cfg.ochre_class.clone(), typed)?;
    Ok(())
}

/// Collect every loop ID currently assigned across all equipment typed configs.
///
/// Used by the dwelling constructor to validate that fluid port declarations
/// reference loop IDs that were actually allocated — catching equipment
/// constructors that hardcode a loop ID instead of using their typed config.
pub(crate) fn collect_allocated_loop_ids(
    specs: &[EquipmentSpec],
) -> std::collections::HashSet<u16> {
    iter_allocated_loop_ids(specs).collect()
}

/// Centralized loop ID allocator.
///
/// Runs after all equipment specs are finalized and the wiring pass has
/// assigned IDs for cross-referenced combi pairs.  For each equipment
/// spec where the typed config's loop ID field is still `None`, assigns
/// the next available ID above the wired range.
///
/// Standalone equipment that shares a fluid loop domain will receive
/// distinct IDs, eliminating collision with wired combi-pair IDs.
pub(crate) fn allocate_loop_ids(specs: &mut [EquipmentSpec]) -> Result<(), HaresError> {
    let max_wired = max_wired_loop_id(specs);
    // When max_wired == u16::MAX (extremely unlikely), saturating_add
    // keeps it at u16::MAX to avoid wrap; every further saturating_add
    // will stay there.  In practice, the allocator will have assigned
    // every loop ID long before this point.
    let mut next_id: u16 = max_wired.saturating_add(1);

    for spec in specs.iter_mut() {
        match spec.name.as_str() {
            "Gas Boiler" => {
                replace_typed::<GasBoilerConfig>(
                    &mut spec.typed_config,
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id.is_none(),
                )?;
            }
            "Electric Boiler" => {
                replace_typed::<ElectricBoilerConfig>(
                    &mut spec.typed_config,
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id.is_none(),
                )?;
            }
            "Gas Water Heater" => {
                replace_typed::<GasWaterHeaterConfig>(
                    &mut spec.typed_config,
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id.is_none(),
                )?;
            }
            "Electric Resistance Water Heater" => {
                replace_typed::<ElectricResistanceWaterHeaterConfig>(
                    &mut spec.typed_config,
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id.is_none(),
                )?;
            }
            "Tankless Water Heater" => {
                replace_typed::<TanklessWaterHeaterConfig>(
                    &mut spec.typed_config,
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id.is_none(),
                )?;
            }
            "Heat Pump Water Heater" => {
                replace_typed::<HeatPumpWaterHeaterConfig>(
                    &mut spec.typed_config,
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id.is_none(),
                )?;
            }
            "Indirect Tank" => {
                replace_typed::<IndirectTankConfig>(
                    &mut spec.typed_config,
                    &mut next_id,
                    |c, id| c.boiler_loop_id = Some(id),
                    |c| c.boiler_loop_id.is_none(),
                )?;
            }
            "Gas Generator" | "Gas Fuel Cell" => {
                replace_typed::<GeneratorConfig>(
                    &mut spec.typed_config,
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id.is_none(),
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hares_equipment::{
        ElectricBoilerConfig, ElectricResistanceWaterHeaterConfig, EquipmentConfig,
        GasBoilerConfig, GasWaterHeaterConfig, HeatPumpWaterHeaterConfig, IndirectTankConfig,
        TanklessWaterHeaterConfig,
    };
    use hares_types::FuelType;

    fn typed_spec<T: EquipmentTypedConfig>(name: &str, config: T) -> EquipmentSpec {
        EquipmentSpec {
            name: name.to_string(),
            instance_name: None,
            fuel_type: FuelType::None,
            parameters: Default::default(),
            zip_params: None,
            typed_config: Some(
                EquipmentConfig::from_typed(name.to_string(), name.to_string(), config).unwrap(),
            ),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    #[test]
    fn empty_specs_produce_zero_max_wired() {
        let specs: Vec<EquipmentSpec> = vec![];
        assert_eq!(max_wired_loop_id(&specs), 0);
    }

    #[test]
    fn all_none_loop_ids_get_sequential_from_one() {
        let mut specs = vec![
            typed_spec(
                "Gas Boiler",
                GasBoilerConfig {
                    loop_id: None,
                    capacity_w: 10000.0,
                    afue: 0.85,
                    ..Default::default()
                },
            ),
            typed_spec(
                "Electric Boiler",
                ElectricBoilerConfig {
                    loop_id: None,
                    capacity_w: 5000.0,
                    eir: 1.0,
                    ..Default::default()
                },
            ),
            typed_spec(
                "Gas Water Heater",
                GasWaterHeaterConfig {
                    fan_power_w: None,
                    loop_id: None,
                    fuel_type: FuelType::Gas,
                    equipment_id: None,
                    zone_id: None,
                    tank_volume_m3: None,
                    tank_height_m: None,
                    energy_factor: None,
                    uniform_energy_factor: None,
                    heating_capacity_w: None,
                    ua_w_per_k: None,
                    setpoint_c: None,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    avg_water_draw_l_per_day: None,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    pilot_power_w: None,
                    flue_loss_fraction: None,
                    skin_loss_fraction: None,
                    ignition_type: None,
                    performance_adjustment: None,
                    zone_type: None,
                    first_hour_rating_m3: None,
                    jacket_r_value_m2_k_w: None,
                    conversion_efficiency: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                    pilot_fraction_to_tank: None,
                },
            ),
        ];

        allocate_loop_ids(&mut specs).unwrap();

        let gb_cfg = specs[0]
            .typed_config
            .as_ref()
            .unwrap()
            .typed::<GasBoilerConfig>()
            .unwrap();
        let eb_cfg = specs[1]
            .typed_config
            .as_ref()
            .unwrap()
            .typed::<ElectricBoilerConfig>()
            .unwrap();
        let gwh_cfg = specs[2]
            .typed_config
            .as_ref()
            .unwrap()
            .typed::<GasWaterHeaterConfig>()
            .unwrap();

        assert_eq!(gb_cfg.loop_id, Some(1));
        assert_eq!(eb_cfg.loop_id, Some(2));
        assert_eq!(gwh_cfg.loop_id, Some(3));
    }

    #[test]
    fn already_wired_ids_are_preserved_and_standalone_get_new_ids() {
        let mut specs = vec![
            // Combi gas boiler — pre-wired by resolve_loop_wiring
            typed_spec(
                "Gas Boiler",
                GasBoilerConfig {
                    loop_id: Some(1),
                    capacity_w: 10000.0,
                    afue: 0.85,
                    ..Default::default()
                },
            ),
            // Combi indirect tank — same loop as boiler
            typed_spec(
                "Indirect Tank",
                IndirectTankConfig {
                    boiler_loop_id: Some(1),
                    equipment_id: None,
                    zone_id: None,
                    tank_volume_m3: None,
                    tank_height_m: None,
                    ua_w_per_k: None,
                    hx_ua_w_per_k: None,
                    setpoint_c: None,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    draw_flow_rate_kg_s: None,
                    avg_water_draw_l_per_day: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    performance_adjustment: None,
                    zone_type: None,
                    first_hour_rating_m3: None,
                    jacket_r_value_m2_k_w: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                    boiler_loop_flow_rate_kg_s: None,
                },
            ),
            // Standalone electric boiler — should get ID 2
            typed_spec(
                "Electric Boiler",
                ElectricBoilerConfig {
                    loop_id: None,
                    capacity_w: 5000.0,
                    eir: 1.0,
                    ..Default::default()
                },
            ),
            // Standalone gas WH — should get ID 3
            typed_spec(
                "Gas Water Heater",
                GasWaterHeaterConfig {
                    fan_power_w: None,
                    loop_id: None,
                    fuel_type: FuelType::Gas,
                    equipment_id: None,
                    zone_id: None,
                    tank_volume_m3: None,
                    tank_height_m: None,
                    energy_factor: None,
                    uniform_energy_factor: None,
                    heating_capacity_w: None,
                    ua_w_per_k: None,
                    setpoint_c: None,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    avg_water_draw_l_per_day: None,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    pilot_power_w: None,
                    flue_loss_fraction: None,
                    skin_loss_fraction: None,
                    ignition_type: None,
                    performance_adjustment: None,
                    zone_type: None,
                    first_hour_rating_m3: None,
                    jacket_r_value_m2_k_w: None,
                    conversion_efficiency: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                    pilot_fraction_to_tank: None,
                },
            ),
        ];

        allocate_loop_ids(&mut specs).unwrap();

        // Wired IDs preserved
        assert_eq!(
            specs[0]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<GasBoilerConfig>()
                .unwrap()
                .loop_id,
            Some(1)
        );
        assert_eq!(
            specs[1]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<IndirectTankConfig>()
                .unwrap()
                .boiler_loop_id,
            Some(1)
        );

        // Standalone equipment gets IDs above wired range
        assert_eq!(
            specs[2]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<ElectricBoilerConfig>()
                .unwrap()
                .loop_id,
            Some(2)
        );
        assert_eq!(
            specs[3]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<GasWaterHeaterConfig>()
                .unwrap()
                .loop_id,
            Some(3)
        );
    }

    #[test]
    fn standalone_equipment_on_separate_circuits_get_distinct_ids() {
        // Scenario from the ticket: standalone gas boiler + combi pair
        let mut specs = vec![
            // Standalone gas boiler — no wiring
            typed_spec(
                "Gas Boiler",
                GasBoilerConfig {
                    loop_id: None,
                    capacity_w: 10000.0,
                    afue: 0.85,
                    ..Default::default()
                },
            ),
            // Combi gas boiler — wired by resolve_loop_wiring
            typed_spec(
                "Gas Boiler",
                GasBoilerConfig {
                    loop_id: Some(1),
                    capacity_w: 15000.0,
                    afue: 0.90,
                    ..Default::default()
                },
            ),
            // Combi indirect tank
            typed_spec(
                "Indirect Tank",
                IndirectTankConfig {
                    boiler_loop_id: Some(1),
                    equipment_id: None,
                    zone_id: None,
                    tank_volume_m3: None,
                    tank_height_m: None,
                    ua_w_per_k: None,
                    hx_ua_w_per_k: None,
                    setpoint_c: None,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    draw_flow_rate_kg_s: None,
                    avg_water_draw_l_per_day: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    performance_adjustment: None,
                    zone_type: None,
                    first_hour_rating_m3: None,
                    jacket_r_value_m2_k_w: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                    boiler_loop_flow_rate_kg_s: None,
                },
            ),
        ];

        allocate_loop_ids(&mut specs).unwrap();

        // Standalone gets ID 2 (above wired ID 1)
        assert_eq!(
            specs[0]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<GasBoilerConfig>()
                .unwrap()
                .loop_id,
            Some(2)
        );
        // Combi boiler keeps ID 1
        assert_eq!(
            specs[1]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<GasBoilerConfig>()
                .unwrap()
                .loop_id,
            Some(1)
        );
        // Combi indirect tank keeps ID 1
        assert_eq!(
            specs[2]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<IndirectTankConfig>()
                .unwrap()
                .boiler_loop_id,
            Some(1)
        );

        // No two equipment share the same loop ID unless they're on the
        // same hydronic loop (the combi pair shares ID 1 by design).
        let ids: Vec<u16> = specs
            .iter()
            .filter_map(|s| match s.name.as_str() {
                "Gas Boiler" => {
                    s.typed_config
                        .as_ref()?
                        .typed::<GasBoilerConfig>()
                        .ok()?
                        .loop_id
                }
                "Indirect Tank" => {
                    s.typed_config
                        .as_ref()?
                        .typed::<IndirectTankConfig>()
                        .ok()?
                        .boiler_loop_id
                }
                _ => None,
            })
            .collect();
        // IDs: [2, 1, 1] — standalone at 2, combi pair at 1
        let mut unique: Vec<u16> = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        // Combi pair (1) appears twice by design; standalone (2) is unique
        assert_eq!(unique.len(), 2);
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
    }

    #[test]
    fn spec_without_typed_config_is_skipped() {
        let mut specs = vec![EquipmentSpec {
            name: "Gas Boiler".to_string(),
            instance_name: None,
            fuel_type: FuelType::None,
            parameters: Default::default(),
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }];

        allocate_loop_ids(&mut specs).unwrap();
        // No panic, no modification (no typed config to patch)
        assert!(specs[0].typed_config.is_none());
    }

    #[test]
    fn all_water_heater_types_get_distinct_ids() {
        let mut specs = vec![
            typed_spec(
                "Gas Water Heater",
                GasWaterHeaterConfig {
                    fan_power_w: None,
                    loop_id: None,
                    fuel_type: FuelType::Gas,
                    equipment_id: None,
                    zone_id: None,
                    tank_volume_m3: None,
                    tank_height_m: None,
                    energy_factor: None,
                    uniform_energy_factor: None,
                    heating_capacity_w: None,
                    ua_w_per_k: None,
                    setpoint_c: None,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    avg_water_draw_l_per_day: None,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    pilot_power_w: None,
                    flue_loss_fraction: None,
                    skin_loss_fraction: None,
                    ignition_type: None,
                    performance_adjustment: None,
                    zone_type: None,
                    first_hour_rating_m3: None,
                    jacket_r_value_m2_k_w: None,
                    conversion_efficiency: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                    pilot_fraction_to_tank: None,
                },
            ),
            typed_spec(
                "Electric Resistance Water Heater",
                ElectricResistanceWaterHeaterConfig {
                    loop_id: None,
                    equipment_id: None,
                    zone_id: None,
                    tank_volume_m3: None,
                    tank_height_m: None,
                    energy_factor: None,
                    uniform_energy_factor: None,
                    heating_capacity_w: None,
                    ua_w_per_k: None,
                    setpoint_c: None,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    avg_water_draw_l_per_day: None,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    performance_adjustment: None,
                    zone_type: None,
                    first_hour_rating_m3: None,
                    element_power_w: None,
                    max_setpoint_ramp_rate_c_per_min: None,
                    element_priority_mode: None,
                    jacket_r_value_m2_k_w: None,
                    max_combined_power_w: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                },
            ),
            typed_spec(
                "Tankless Water Heater",
                TanklessWaterHeaterConfig {
                    loop_id: None,
                    fuel_type: FuelType::Gas,
                    equipment_id: None,
                    zone_id: None,
                    energy_factor: None,
                    uniform_energy_factor: None,
                    heating_capacity_w: None,
                    setpoint_c: None,
                    parasitic_power_w: None,
                    performance_adjustment: None,
                    inlet_temp_c: None,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    avg_water_draw_l_per_day: None,
                    zone_type: None,
                    min_flow_kg_s: None,
                    min_flow_gpm: None,
                },
            ),
            typed_spec(
                "Heat Pump Water Heater",
                HeatPumpWaterHeaterConfig {
                    loop_id: None,
                    equipment_id: None,
                    zone_id: None,
                    tank_volume_m3: None,
                    tank_height_m: None,
                    cop: None,
                    backup_element_power_w: None,
                    ua_w_per_k: None,
                    setpoint_c: None,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    tempering_valve_setpoint_c: None,
                    avg_water_draw_l_per_day: None,
                    draw_flow_rate_kg_s: None,
                    compressor_power_w: None,
                    backup_enable_offset_c: None,
                    min_ambient_temp_c: None,
                    max_ambient_temp_c: None,
                    min_on_time_s: None,
                    min_off_time_s: None,
                    hp_only_mode: None,
                    element_hp_control_mode: None,
                    fan_power_w: None,
                    parasitic_power_w: None,
                    backup_efficiency: None,
                    shr: None,
                    lost_heat_fraction: None,
                    wall_heat_fraction: None,
                    capacity_biquadratic_coeffs: None,
                    cop_biquadratic_coeffs: None,
                    performance_adjustment: None,
                    zone_type: None,
                    first_hour_rating_m3: None,
                    jacket_r_value_m2_k_w: None,
                    fixture_delivery_temp_c: None,
                },
            ),
        ];

        allocate_loop_ids(&mut specs).unwrap();

        let ids: Vec<u16> = vec![
            specs[0]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<GasWaterHeaterConfig>()
                .unwrap()
                .loop_id
                .unwrap(),
            specs[1]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<ElectricResistanceWaterHeaterConfig>()
                .unwrap()
                .loop_id
                .unwrap(),
            specs[2]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<TanklessWaterHeaterConfig>()
                .unwrap()
                .loop_id
                .unwrap(),
            specs[3]
                .typed_config
                .as_ref()
                .unwrap()
                .typed::<HeatPumpWaterHeaterConfig>()
                .unwrap()
                .loop_id
                .unwrap(),
        ];

        // All IDs must be distinct
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            4,
            "all 4 water heaters must have distinct loop IDs"
        );
        // Sequential starting from 1
        assert_eq!(sorted, vec![1, 2, 3, 4]);
    }
}
