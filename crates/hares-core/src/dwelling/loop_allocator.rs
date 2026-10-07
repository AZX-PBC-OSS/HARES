//! Centralized fluid loop ID allocator for equipment specs.
//!
//! After `resolve_loop_wiring` assigns IDs to cross-referenced combi pairs,
//! this pass scans all typed configs, finds the maximum already-assigned loop
//! ID, and assigns unique sequential IDs to equipment whose typed config
//! carries a `None` loop_id (or `boiler_loop_id` for indirect tanks). A spec
//! pending autosizing (typed config `None`) is not skipped: its ID is
//! written into its raw parameters alone.
//!
//! Every assigned or wired ID is mirrored into the spec's raw parameters
//! under the key the post-autosize typed-config rebuild reads (`loop_id`
//! for boilers and water heaters, `boiler_loop_id` for indirect tanks), so
//! autosizing a spec never destroys the ID it was wired or allocated. The
//! generator arm stays typed-config-only: no rebuild path reads generator
//! parameters.
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

/// All allocated loop IDs across equipment typed configs.
///
/// Collects the `loop_id` (or `boiler_loop_id` for indirect tanks) from every
/// equipment spec whose typed config carries a `Some` value; a typed config
/// that does not read as its spec's config type is an error naming the
/// spec, not a spec without a loop id.  This is the
/// single source of truth for which equipment types carry loop IDs — both
/// `max_wired_loop_id` and `collect_allocated_loop_ids` derive from it,
/// guaranteeing that adding a new equipment type to this match arm
/// automatically covers allocation, validation, and max-finding.
fn allocated_loop_ids(specs: &[EquipmentSpec]) -> Result<Vec<u16>, HaresError> {
    let mut ids = Vec::new();
    for spec in specs {
        let Some(cfg) = spec.typed_config.as_ref() else {
            continue;
        };
        let label = spec.name.as_str();
        let id = match label {
            "Gas Boiler" => cfg.require_typed::<GasBoilerConfig>(label)?.loop_id,
            "Electric Boiler" => cfg.require_typed::<ElectricBoilerConfig>(label)?.loop_id,
            "Gas Water Heater" => cfg.require_typed::<GasWaterHeaterConfig>(label)?.loop_id,
            "Electric Resistance Water Heater" => {
                cfg.require_typed::<ElectricResistanceWaterHeaterConfig>(label)?
                    .loop_id
            }
            "Tankless Water Heater" => {
                cfg.require_typed::<TanklessWaterHeaterConfig>(label)?
                    .loop_id
            }
            "Heat Pump Water Heater" => {
                cfg.require_typed::<HeatPumpWaterHeaterConfig>(label)?
                    .loop_id
            }
            "Indirect Tank" => {
                cfg.require_typed::<IndirectTankConfig>(label)?
                    .boiler_loop_id
            }
            "Gas Generator" | "Gas Fuel Cell" => {
                cfg.require_typed::<GeneratorConfig>(label)?.loop_id
            }
            _ => None,
        };
        ids.extend(id);
    }
    Ok(ids)
}

/// Scan all equipment typed configs and find the maximum assigned loop ID.
///
/// Loop ID 0 (the `Default` for `LoopId`) is never explicitly assigned by
/// `resolve_loop_wiring` (which starts from 1).  Returning 0 when no IDs are
/// wired produces the correct `max_wired + 1 = 1` start for the allocator.
fn max_wired_loop_id(specs: &[EquipmentSpec]) -> Result<u16, HaresError> {
    Ok(allocated_loop_ids(specs)?.into_iter().max().unwrap_or(0))
}

/// Hands out the next free loop id for `spec_name`. Ids stop below the
/// shared DHW demand loop's reserved id: running out is an error naming
/// the spec left without one, never a repeat of an id already in use.
fn take_loop_id(next_id: &mut u32, spec_name: &str) -> Result<u16, HaresError> {
    let reserved = hares_equipment::DHW_DEMAND_LOOP.0;
    let id = u16::try_from(*next_id)
        .ok()
        .filter(|id| *id < reserved)
        .ok_or_else(|| {
            HaresError::Dwelling(format!(
                "no fluid loop id remains for '{spec_name}': allocated ids stop below \
                 the reserved DHW demand loop id {reserved}"
            ))
        })?;
    *next_id += 1;
    Ok(id)
}

/// Patches a typed config in place when its loop id is `None`, and returns
/// the loop id the config carries afterwards.
fn replace_typed<T: EquipmentTypedConfig>(
    cfg: &mut EquipmentConfig,
    spec_name: &str,
    next_id: &mut u32,
    set: impl FnOnce(&mut T, u16),
    get: impl Fn(&T) -> Option<u16>,
) -> Result<u16, HaresError> {
    let mut typed = cfg.require_typed::<T>(spec_name)?;
    if let Some(id) = get(&typed) {
        return Ok(id);
    }
    let id = take_loop_id(next_id, spec_name)?;
    set(&mut typed, id);
    *cfg = EquipmentConfig::from_typed(cfg.name.clone(), cfg.ochre_class.clone(), typed)?;
    Ok(id)
}

/// Collect every loop ID currently assigned across all equipment typed configs.
///
/// Used by the dwelling constructor to validate that fluid port declarations
/// reference loop IDs that were actually allocated — catching equipment
/// constructors that hardcode a loop ID instead of using their typed config.
pub(crate) fn collect_allocated_loop_ids(
    specs: &[EquipmentSpec],
) -> Result<std::collections::HashSet<u16>, HaresError> {
    Ok(allocated_loop_ids(specs)?.into_iter().collect())
}

/// The loop id a spec's raw parameters carry under `key` (written there by
/// the wiring pass, or by this allocator for equipment pending autosizing).
/// A present value that is not a u16 is an error naming it.
fn param_loop_id(spec: &EquipmentSpec, key: &str) -> Result<Option<u16>, HaresError> {
    let Some(value) = spec.parameters.get(key) else {
        return Ok(None);
    };
    value
        .as_u64()
        .and_then(|v| u16::try_from(v).ok())
        .map(Some)
        .ok_or_else(|| {
            HaresError::Dwelling(format!(
                "equipment '{}' carries {key} = {value} in its parameters, which is not a \
                 loop id (an integer from 0 to {})",
                spec.name,
                u16::MAX
            ))
        })
}

/// Allocate one spec's loop id under `param_key`. The id travels in the
/// spec's raw parameters (which the post-autosize typed-config rebuild
/// reads) as well as the typed config: a spec pending autosizing (typed
/// config `None`) carries the id in its parameters alone, and a
/// typed-present spec's assigned-or-wired id is mirrored into the
/// parameters so the same rebuild keeps it.
fn allocate_loop_id<T: EquipmentTypedConfig>(
    spec: &mut EquipmentSpec,
    param_key: &str,
    next_id: &mut u32,
    set: impl FnOnce(&mut T, u16),
    get: impl Fn(&T) -> Option<u16>,
) -> Result<(), HaresError> {
    let id = match spec.typed_config.as_mut() {
        Some(cfg) => replace_typed::<T>(cfg, &spec.name, next_id, set, get)?,
        None => match param_loop_id(spec, param_key)? {
            Some(_) => return Ok(()),
            None => take_loop_id(next_id, &spec.name)?,
        },
    };
    spec.parameters
        .insert(param_key.to_string(), serde_json::json!(id));
    Ok(())
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
    let mut next_id = u32::from(max_wired_loop_id(specs)?) + 1;

    for spec in specs.iter_mut() {
        match spec.name.as_str() {
            "Gas Boiler" => {
                allocate_loop_id::<GasBoilerConfig>(
                    spec,
                    "loop_id",
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id,
                )?;
            }
            "Electric Boiler" => {
                allocate_loop_id::<ElectricBoilerConfig>(
                    spec,
                    "loop_id",
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id,
                )?;
            }
            "Gas Water Heater" => {
                allocate_loop_id::<GasWaterHeaterConfig>(
                    spec,
                    "loop_id",
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id,
                )?;
            }
            "Electric Resistance Water Heater" => {
                allocate_loop_id::<ElectricResistanceWaterHeaterConfig>(
                    spec,
                    "loop_id",
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id,
                )?;
            }
            "Tankless Water Heater" => {
                allocate_loop_id::<TanklessWaterHeaterConfig>(
                    spec,
                    "loop_id",
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id,
                )?;
            }
            "Heat Pump Water Heater" => {
                allocate_loop_id::<HeatPumpWaterHeaterConfig>(
                    spec,
                    "loop_id",
                    &mut next_id,
                    |c, id| c.loop_id = Some(id),
                    |c| c.loop_id,
                )?;
            }
            "Indirect Tank" => {
                allocate_loop_id::<IndirectTankConfig>(
                    spec,
                    "boiler_loop_id",
                    &mut next_id,
                    |c, id| c.boiler_loop_id = Some(id),
                    |c| c.boiler_loop_id,
                )?;
            }
            "Gas Generator" | "Gas Fuel Cell" => {
                if let Some(cfg) = spec.typed_config.as_mut() {
                    replace_typed::<GeneratorConfig>(
                        cfg,
                        &spec.name,
                        &mut next_id,
                        |c, id| c.loop_id = Some(id),
                        |c| c.loop_id,
                    )?;
                }
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
            typed_overrides: serde_json::Map::new(),
            typed_config: Some(
                EquipmentConfig::from_typed(name.to_string(), name.to_string(), config).unwrap(),
            ),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    fn pending_spec(
        name: &str,
        parameters: serde_json::Map<String, serde_json::Value>,
    ) -> EquipmentSpec {
        EquipmentSpec {
            name: name.to_string(),
            instance_name: None,
            fuel_type: FuelType::None,
            parameters,
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    fn spec_params(
        pairs: &[(&str, serde_json::Value)],
    ) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    fn indirect_tank_config(boiler_loop_id: Option<u16>) -> IndirectTankConfig {
        IndirectTankConfig {
            boiler_loop_id,
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
        }
    }

    fn gas_water_heater_config(loop_id: Option<u16>) -> GasWaterHeaterConfig {
        GasWaterHeaterConfig {
            fan_power_w: None,
            loop_id,
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
        }
    }

    fn electric_resistance_water_heater_config(
        loop_id: Option<u16>,
    ) -> ElectricResistanceWaterHeaterConfig {
        ElectricResistanceWaterHeaterConfig {
            loop_id,
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
        }
    }

    fn tankless_water_heater_config(loop_id: Option<u16>) -> TanklessWaterHeaterConfig {
        TanklessWaterHeaterConfig {
            loop_id,
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
        }
    }

    fn heat_pump_water_heater_config(loop_id: Option<u16>) -> HeatPumpWaterHeaterConfig {
        HeatPumpWaterHeaterConfig {
            loop_id,
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
            cop_curve_is_normalized: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            low_power_hpwh: None,
            uniform_energy_factor: None,
        }
    }

    #[test]
    fn empty_specs_produce_zero_max_wired() {
        let specs: Vec<EquipmentSpec> = vec![];
        assert_eq!(max_wired_loop_id(&specs).unwrap(), 0);
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

    /// A typed-`None` spec is not skipped: it is pending autosizing, so
    /// its loop id lives in its raw parameters for the post-autosize
    /// typed-config rebuild to read. The pinned counter semantics: a
    /// pending boiler whose parameters already carry the wiring pass's id
    /// is preserved without consuming the counter, and a second pending
    /// boiler gets the next id.
    #[test]
    fn pending_boilers_carry_their_loop_id_in_params_and_wired_ids_do_not_consume_the_counter() {
        let mut specs = vec![
            // The wired pair's tank: its typed config carries the wired id
            // and bounds the allocator's floor above it.
            typed_spec("Indirect Tank", indirect_tank_config(Some(1))),
            // The wired pair's boiler, pending autosizing: typed config
            // `None`, the wiring pass's id in its parameters.
            pending_spec(
                "Gas Boiler",
                spec_params(&[("loop_id", serde_json::json!(1))]),
            ),
            // A second pending boiler with no id yet.
            pending_spec("Electric Boiler", spec_params(&[])),
            // A typed-present standalone boiler with no id yet.
            typed_spec(
                "Gas Boiler",
                GasBoilerConfig {
                    loop_id: None,
                    capacity_w: 10000.0,
                    afue: 0.85,
                    ..Default::default()
                },
            ),
        ];

        allocate_loop_ids(&mut specs).unwrap();

        // The wired-pending boiler is preserved without consuming the
        // counter: the next id is not skipped.
        assert!(specs[1].typed_config.is_none());
        assert_eq!(
            specs[1].parameters.get("loop_id"),
            Some(&serde_json::json!(1))
        );
        // The second pending boiler gets the next id, in its parameters
        // alone.
        assert!(specs[2].typed_config.is_none());
        assert_eq!(
            specs[2].parameters.get("loop_id"),
            Some(&serde_json::json!(2))
        );
        // The typed-present boiler takes the id after that, mirrored into
        // its parameters too.
        let standalone = specs[3]
            .typed_config
            .as_ref()
            .unwrap()
            .typed::<GasBoilerConfig>()
            .unwrap();
        assert_eq!(standalone.loop_id, Some(3));
        assert_eq!(
            specs[3].parameters.get("loop_id"),
            Some(&serde_json::json!(3))
        );
    }

    /// The water-heater and indirect-tank arms mirror their assigned id
    /// into the spec's raw parameters under the key the post-autosize
    /// WH-typed-config rebuild reads: `loop_id` for the water-heater
    /// classes, `boiler_loop_id` for the indirect tank. Without the mirror
    /// the rebuild rebuilds each config from the stale parameters and
    /// destroys the id the allocator just assigned.
    #[test]
    fn water_heater_and_tank_arms_mirror_assigned_ids_into_params() {
        let mut specs = vec![
            typed_spec("Gas Water Heater", gas_water_heater_config(None)),
            typed_spec(
                "Electric Resistance Water Heater",
                electric_resistance_water_heater_config(None),
            ),
            typed_spec("Tankless Water Heater", tankless_water_heater_config(None)),
            typed_spec(
                "Heat Pump Water Heater",
                heat_pump_water_heater_config(None),
            ),
            typed_spec("Indirect Tank", indirect_tank_config(None)),
        ];

        allocate_loop_ids(&mut specs).unwrap();

        for (idx, (name, key)) in [
            ("Gas Water Heater", "loop_id"),
            ("Electric Resistance Water Heater", "loop_id"),
            ("Tankless Water Heater", "loop_id"),
            ("Heat Pump Water Heater", "loop_id"),
            ("Indirect Tank", "boiler_loop_id"),
        ]
        .into_iter()
        .enumerate()
        {
            let id = (idx + 1) as u16;
            assert_eq!(
                specs[idx].parameters.get(key),
                Some(&serde_json::json!(id)),
                "{name} must mirror its assigned id into '{key}' in the parameters"
            );
        }
    }

    /// A wired id reaches the parameters mirror without consuming the
    /// counter: the indirect tank below was wired to its boiler on loop 1,
    /// so the standalone water heater takes the next id, not the one after.
    #[test]
    fn indirect_tank_wired_id_is_mirrored_into_params_without_consuming_the_counter() {
        let mut specs = vec![
            typed_spec("Indirect Tank", indirect_tank_config(Some(1))),
            typed_spec("Gas Water Heater", gas_water_heater_config(None)),
        ];

        allocate_loop_ids(&mut specs).unwrap();

        assert_eq!(
            specs[0].parameters.get("boiler_loop_id"),
            Some(&serde_json::json!(1))
        );
        let tank = specs[0]
            .typed_config
            .as_ref()
            .unwrap()
            .typed::<IndirectTankConfig>()
            .unwrap();
        assert_eq!(tank.boiler_loop_id, Some(1), "the wired id is preserved");
        assert_eq!(
            specs[1].parameters.get("loop_id"),
            Some(&serde_json::json!(2))
        );
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
                    cop_curve_is_normalized: None,
                    performance_adjustment: None,
                    zone_type: None,
                    first_hour_rating_m3: None,
                    jacket_r_value_m2_k_w: None,
                    fixture_delivery_temp_c: None,
                    low_power_hpwh: None,
                    uniform_energy_factor: None,
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

    /// A loop id already in a pending spec's parameters that is not a u16
    /// is an error naming the spec, the key and the value, never read as
    /// absent and overwritten with a fresh id.
    #[test]
    fn malformed_loop_id_in_params_is_an_error() {
        for bad in [
            serde_json::json!(-1),
            serde_json::json!(70_000),
            serde_json::json!("two"),
        ] {
            let mut specs = vec![pending_spec(
                "Gas Boiler",
                spec_params(&[("loop_id", bad.clone())]),
            )];
            let err = allocate_loop_ids(&mut specs)
                .expect_err("a malformed loop id must not be replaced");
            let msg = err.to_string();
            assert!(
                msg.contains("Gas Boiler")
                    && msg.contains("loop_id")
                    && msg.contains(&bad.to_string()),
                "the error names the spec, the key and the value, got: {msg}"
            );
            assert_eq!(specs[0].parameters.get("loop_id"), Some(&bad));
        }
    }

    /// A typed config that does not read as its spec's config type is an
    /// error naming the spec, never skipped as if it carried no loop id.
    #[test]
    fn unreadable_typed_config_is_an_error() {
        let mut specs = vec![typed_spec("Gas Boiler", gas_water_heater_config(Some(1)))];
        let err = allocate_loop_ids(&mut specs)
            .expect_err("a config that does not read as a Gas Boiler config must fail");
        assert!(
            err.to_string().contains("Gas Boiler"),
            "the error names the spec, got: {err}"
        );
    }

    /// Ids past u16::MAX are an error, never a repeat of the last id.
    #[test]
    fn exhausted_loop_ids_are_an_error() {
        let mut specs = vec![
            typed_spec("Indirect Tank", indirect_tank_config(Some(u16::MAX))),
            pending_spec("Gas Boiler", spec_params(&[])),
        ];
        let err = allocate_loop_ids(&mut specs).expect_err("no id remains above the wired maximum");
        assert!(
            err.to_string().contains("Gas Boiler"),
            "the error names the spec left without an id, got: {err}"
        );
        assert!(specs[1].parameters.get("loop_id").is_none());
    }
}
