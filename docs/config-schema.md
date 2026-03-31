# Config Schema Reference

This document lists the production Rust structs that implement `EquipmentTypedConfig` in `crates/hares-equipment`.
It is the reference for typed built-in equipment payloads and for Python callers that need to build configs compatible with built-in equipment.

Conventions:
- `equipment_id` and `zone_id` are optional identity fields unless noted otherwise.
- `loop_id` is an optional hydronic / DHW loop identifier unless noted otherwise.
- `Option<T>` fields are optional.
- Non-`Option` fields are required unless the struct uses a serde default for that field.
- `DuctConfig` is flattened into the HVAC structs that include ducts.

## Shared Flattened Struct

### `DuctConfig`

Used by: `GasFurnaceConfig`, `ElectricFurnaceConfig`, `CentralAirConditionerConfig`, `HeatPumpHeaterConfig`, `HeatPumpCoolerConfig`

All fields are optional: `dse_heat`, `dse_cool`, `duct_zone_id`, `duct_house_volume_m3`, `duct_supply_leakage_frac`, `duct_supply_area_m2`, `duct_supply_r_m2_k_w`, `duct_return_leakage_frac`, `duct_return_area_m2`, `duct_return_r_m2_k_w`, `duct_zone_type`.

## HVAC Heating

| Struct | Typed name | Canonical equipment | Required fields | Optional / defaulted fields |
| --- | --- | --- | --- | --- |
| `GasFurnaceConfig` | `Gas Furnace` | `Gas Furnace` | `capacity_w`, `afue` | `equipment_id`, `zone_id`, `fan_power_w`, `number_of_speeds`, flattened `DuctConfig` |
| `ElectricFurnaceConfig` | `Electric Furnace` | `Electric Furnace` | `capacity_w`, `eir` | `equipment_id`, `zone_id`, `fan_power_w`, `number_of_speeds`, flattened `DuctConfig` |
| `GasBoilerConfig` | `Gas Boiler` | `Gas Boiler` | `capacity_w`, `afue` | `equipment_id`, `zone_id`, `loop_id`, `flow_rate_kg_s`, `return_temp_c`, `fluid_type`, `fan_power_w`, `number_of_speeds` |
| `ElectricBoilerConfig` | `Electric Boiler` | `Electric Boiler` | `capacity_w`, `eir` | `equipment_id`, `zone_id`, `loop_id`, `flow_rate_kg_s`, `return_temp_c`, `fluid_type`, `fan_power_w`, `number_of_speeds` |
| `ElectricBaseboardConfig` | `Electric Baseboard` | `Electric Baseboard` | `capacity_w`, `eir` | `equipment_id`, `zone_id` |
| `IdealHvacConfig` | `Ideal HVAC` | `Ideal HVAC` | None | `equipment_id`, `zone_id`, `heating_capacity_w`, `cooling_capacity_w`, `heating_eir`, `cooling_eir`, `shr`, `fraction_heating_load_served`, `fraction_cooling_load_served` |

## HVAC Cooling

| Struct | Typed name | Canonical equipment | Required fields | Optional / defaulted fields |
| --- | --- | --- | --- | --- |
| `CentralAirConditionerConfig` | `Central AC` | `Air Conditioner` | `capacity_w`, `seer` | `equipment_id`, `zone_id`, `shr`, `number_of_speeds`, `stage_capacities_w`, `stage_eirs`, `stage_shrs`, `fan_power_w`, `fan_power_w_per_cfm`, `fraction_load_served`, flattened `DuctConfig`, `system_type`, `startup_cd`, `biquadratic_x1_min`, `biquadratic_x1_max`, `biquadratic_x2_min`, `biquadratic_x2_max`, `ff_min`, `ff_max`, `plf_min`, `plf_max` |
| `RoomAcConfig` | `Room AC` | `Room AC` | `capacity_w`, `eer` | `equipment_id`, `zone_id`, `biquadratic_x1_min`, `biquadratic_x1_max`, `biquadratic_x2_min`, `biquadratic_x2_max`, `ff_min`, `ff_max`, `plf_min`, `plf_max` |
| `DehumidifierConfig` | `Dehumidifier` | `Dehumidifier` | None | `equipment_id`, `zone_id`, `capacity_liters_per_day`, `energy_factor`, `integrated_energy_factor`, `fraction_served`, `target_rh` |

## Heat Pumps

| Struct | Typed name | Canonical equipment | Required fields | Optional / defaulted fields |
| --- | --- | --- | --- | --- |
| `HeatPumpHeaterConfig` | `ASHP Heater` | `ASHP Heater`, `MSHP Heater` | None | `equipment_id`, `zone_id`, `heating_capacity_w`, `hspf`, `stage_heating_capacities_w`, `stage_heating_eirs`, `backup_fuel`, `backup_capacity_w`, `backup_eir`, `fraction_heating_load_served`, `cooling_capacity_w`, `seer`, `stage_cooling_capacities_w`, `stage_cooling_eirs`, `stage_shrs`, `fraction_cooling_load_served`, `number_of_speeds` (defaults to `1`), `is_mini_split` (defaults to `false`), `shr`, `fan_power_w`, `fan_power_w_per_cfm`, flattened `DuctConfig`, `biquadratic_x1_min`, `biquadratic_x1_max`, `biquadratic_x2_min`, `biquadratic_x2_max`, `ff_min`, `ff_max`, `plf_min`, `plf_max` |
| `HeatPumpCoolerConfig` | `ASHP Cooler` | `ASHP Cooler`, `MSHP Cooler` | None | Same field set as `HeatPumpHeaterConfig`: `equipment_id`, `zone_id`, `heating_capacity_w`, `hspf`, `stage_heating_capacities_w`, `stage_heating_eirs`, `backup_fuel`, `backup_capacity_w`, `backup_eir`, `fraction_heating_load_served`, `cooling_capacity_w`, `seer`, `stage_cooling_capacities_w`, `stage_cooling_eirs`, `stage_shrs`, `fraction_cooling_load_served`, `number_of_speeds`, `is_mini_split`, `shr`, `fan_power_w`, `fan_power_w_per_cfm`, flattened `DuctConfig`, `biquadratic_x1_min`, `biquadratic_x1_max`, `biquadratic_x2_min`, `biquadratic_x2_max`, `ff_min`, `ff_max`, `plf_min`, `plf_max` |

## Water Heating

| Struct | Typed name | Canonical equipment | Required fields | Optional / defaulted fields |
| --- | --- | --- | --- | --- |
| `GasWaterHeaterConfig` | `Gas Water Heater` | `Gas Water Heater` | `fuel_type` | `equipment_id`, `zone_id`, `loop_id`, `tank_volume_m3`, `tank_height_m`, `energy_factor`, `uniform_energy_factor`, `heating_capacity_w`, `ua_w_per_k`, `setpoint_c`, `deadband_c`, `max_tank_temp_c`, `initial_tank_temp_c`, `tank_nodes`, `avg_water_draw_l_per_day`, `draw_flow_rate_kg_s`, `pilot_power_w`, `flue_loss_fraction`, `skin_loss_fraction`, `ignition_type`, `performance_adjustment`, `zone_type`, `first_hour_rating_m3` |
| `ElectricResistanceWaterHeaterConfig` | `Electric Resistance Water Heater` | `Electric Resistance Water Heater` | None | `equipment_id`, `zone_id`, `loop_id`, `tank_volume_m3`, `tank_height_m`, `energy_factor`, `uniform_energy_factor`, `heating_capacity_w`, `ua_w_per_k`, `setpoint_c`, `deadband_c`, `max_tank_temp_c`, `initial_tank_temp_c`, `tank_nodes`, `avg_water_draw_l_per_day`, `draw_flow_rate_kg_s`, `performance_adjustment`, `zone_type`, `first_hour_rating_m3`, `element_power_w`, `element_priority_mode` |
| `TanklessWaterHeaterConfig` | `Tankless Water Heater` | `Tankless Water Heater` | `fuel_type` | `equipment_id`, `zone_id`, `loop_id`, `energy_factor`, `uniform_energy_factor`, `heating_capacity_w`, `setpoint_c`, `parasitic_power_w`, `performance_adjustment`, `inlet_temp_c`, `draw_flow_rate_kg_s`, `avg_water_draw_l_per_day` |
| `HeatPumpWaterHeaterConfig` | `Heat Pump Water Heater` | `Heat Pump Water Heater` | None | `equipment_id`, `zone_id`, `loop_id`, `tank_volume_m3`, `tank_height_m`, `cop`, `backup_element_power_w`, `ua_w_per_k`, `setpoint_c`, `deadband_c`, `max_tank_temp_c`, `initial_tank_temp_c`, `tank_nodes`, `tempering_valve_setpoint_c`, `avg_water_draw_l_per_day`, `draw_flow_rate_kg_s`, `compressor_power_w`, `backup_enable_offset_c`, `min_ambient_temp_c`, `max_ambient_temp_c`, `min_on_time_s`, `min_off_time_s`, `hp_only_mode`, `element_hp_control_mode`, `fan_power_w`, `parasitic_power_w`, `backup_efficiency`, `shr`, `lost_heat_fraction`, `wall_heat_fraction`, `capacity_biquadratic_coeffs`, `cop_biquadratic_coeffs`, `performance_adjustment`, `zone_type`, `first_hour_rating_m3` |

## Battery, EV, PV, Generator, Ventilation

| Struct | Typed name | Canonical equipment | Required fields | Optional / defaulted fields |
| --- | --- | --- | --- | --- |
| `BatteryConfig` | `Battery` | `Battery` | `capacity_kwh`, `max_charge_kw`, `max_discharge_kw` | `equipment_id`, `zone_id`, `n_series`, `n_parallel`, `ah_cell`, `v_cell`, `cell_resistance_ohm`, `pack_voltage_v`, `chemistry`, `standby_power_w`, `self_discharge_pct_per_day`, `min_soc`, `max_soc`, `initial_soc`, `import_limit_w`, `export_limit_w`, `heater_power_w`, `heater_threshold_c`, `heater_on_discharge`, `min_discharge_temp_c`, `full_power_temp_c`, `min_charge_temp_c`, `cell_thermal_mass_j_per_k`, `cell_ua_w_per_k`, `inverter_efficiency`, `charge_efficiency`, `discharge_efficiency`, `bms_mode`, `grid_export_rule` |
| `EvConfig` | `EV` | `EV` | `capacity_kwh`, `max_charging_power_kw` | `equipment_id`, `charging_level`, `charging_efficiency`, `l1_current_a`, `l1_voltage_v`, `soc_max`, `initial_soc`, `battery_temp_c`, `min_charge_temp_c`, `full_power_temp_c`, `heater_power_w`, `heater_threshold_c`, `thermal_mass_j_per_k`, `ua_w_per_k`, `v2l_enabled`, `v2l_soc_reserve`, `v2l_max_discharge_kw`, `v2g_enabled`, `v2g_soc_reserve`, `v2g_max_discharge_kw`, `chemistry`, `fuel_economy_kwh_per_mi`, `ready_soc`, `charging_strategy`, `plug_in_policy`, `power_limit_kw`, `initial_connection_state` |
| `PvConfig` | `PV` | `PV` | `capacity_kw` | `equipment_id`, `zone_id`, `tilt_deg`, `azimuth_deg`, `module_type`, `noct_c`, `system_losses_fraction`, `inverter_efficiency`, `inverter_capacity_kw`, `power_factor`, `surface_resolution_deg` |
| `GeneratorConfig` | `Generator` | `Gas Generator`, `Gas Fuel Cell` | `rated_power_kw` | `equipment_id`, `fuel_type`, `eta_electric`, `eta_thermal`, `efficiency_type`, `delta_kw_per_s`, `capacity_min_kw`, `grid_import_limit_kw`, `export_limit_kw`, `loop_id`, `flow_rate_kg_s`, `supply_temp_c`, `return_temp_c` |
| `VentilationConfig` | `Ventilation` | `Ventilation` | `flow_rate_m3_s` | `equipment_id`, `zone_id`, `fan_power_w`, `sensible_effectiveness`, `latent_effectiveness`, `bypass_temp_min_c`, `bypass_temp_max_c`, `defrost_temp_c`, `defrost_effectiveness_fraction`, `ventilation_type`, `balanced`, `hours_in_operation` |

## Notes

- `HeatPumpConfig` is a Rust type alias for `HeatPumpHeaterConfig`; it is not a separate schema.
- Utility structs such as `ThermostatConfig`, `StartupConfig`, `StratifiedTankConfig`, `TemperedDrawConfig`, and `SoilingConfig` do not implement `EquipmentTypedConfig`; they are not standalone equipment payloads and are omitted here.
- `GeneratorConfig` uses the typed payload name `Generator`, but the instantiated equipment label still depends on registry kind (`Gas Generator` vs `Gas Fuel Cell`).
