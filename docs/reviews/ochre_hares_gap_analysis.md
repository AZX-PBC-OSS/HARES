# OCHRE vs HARES Gap Analysis

This document identifies the gap between what OCHRE reads from HPXML files and what HARES currently maps. These are the attributes that need to be added to achieve OCHRE parity.

---

## Missing in HARES (Parity Gap)

### Critical (Equipment may not work correctly without these)

| Attribute | XPATH | Used By OCHRE For | Priority |
|-----------|-------|-------------------|----------|
| WeatherStation/Name | /ClimateandRiskZones/WeatherStation/Name | Weather station lookup for TMY data | HIGH |
| AnnualHeatingDistributionSystemEfficiency | /Systems/HVAC/HVACDistribution/AnnualHeatingDistributionSystemEfficiency | Ducted system heating efficiency (DSE) | HIGH |
| AnnualCoolingDistributionSystemEfficiency | /Systems/HVAC/HVACDistribution/AnnualCoolingDistributionSystemEfficiency | Ducted system cooling efficiency (DSE) | HIGH |
| HeatPumpType (HeatingSystem) | /Systems/HVAC/HeatingSystem/HeatPumpType | Identifies if heating system is a heat pump | HIGH |
| HeatPumpFuel (HeatingSystem) | /Systems/HVAC/HeatingSystem/HeatPumpFuel | Fuel type for heat pump heating | HIGH |
| HeatPumpType (CoolingSystem) | /Systems/HVAC/CoolingSystem/HeatPumpType | Identifies if cooling system is a heat pump | HIGH |
| HeatPumpFuel (CoolingSystem) | /Systems/HVAC/CoolingSystem/CoolingSystemFuel | Fuel type for heat pump cooling | HIGH |
| JacketRValue | /Systems/WaterHeating/WaterHeatingSystem/WaterHeaterInsulation/Jacket/JacketRValue | Water heater standby losses | HIGH |

### Important (May affect simulation accuracy)

| Attribute | XPATH | Used By OCHRE For | Priority |
|-----------|-------|-------------------|----------|
| NumberofBathrooms | /BuildingSummary/BuildingConstruction/NumberofBathrooms | Internal gain calculations (hot water, dryers) | MEDIUM |
| FractionDHWLoadServed | /Systems/WaterHeating/WaterHeatingSystem/FractionDHWLoadServed | Multiple water heaters serving different loads | MEDIUM |
| SensibleHeatFraction (Heating) | /Systems/HVAC/HeatingSystem/SensibleHeatFraction | Heating latent/sensible split | MEDIUM |
| PrimaryIndicator | /Appliances/Refrigerator/PrimaryIndicator | Identifies primary vs secondary refrigerator | MEDIUM |
| CookingRange/Location | /Appliances/CookingRange/Location | Cooking heat gain location | MEDIUM |
| Oven/IsConvection | /Appliances/Oven/IsConvection | Oven efficiency adjustment | MEDIUM |
| Pool/Type | /Pools/Pool/Type | Pool type for detailed modeling | MEDIUM |
| Spa/Type | /Spas/Spa/Type | Spa type for detailed modeling | MEDIUM |

### Nice-to-Have (Lower priority, mostly for detailed schedules)

| Attribute | XPATH | Used By OCHRE For | Priority |
|-----------|-------|-------------------|----------|
| PartitionWallMass/AreaFraction | /Enclosure/extension/PartitionWallMass/AreaFraction | Internal wall thermal mass | LOW |
| PartitionWallMass/InteriorFinish/Type | /Enclosure/extension/PartitionWallMass/InteriorFinish/Type | Internal wall finish type | LOW |
| PartitionWallMass/InteriorFinish/Thickness | /Enclosure/extension/PartitionWallMass/InteriorFinish/Thickness | Internal wall thickness | LOW |
| MonthlyScheduleMultipliers (HVAC) | /HVACPlant/HVACControl/extension/MonthlyScheduleMultipliers | Thermostat schedule monthly variation | LOW |
| WaterFixturesWeekdayScheduleFractions | /WaterHeating/extension/WaterFixturesWeekdayScheduleFractions | DHW draw schedule | LOW |
| WaterFixturesWeekendScheduleFractions | /WaterHeating/extension/WaterFixturesWeekendScheduleFractions | DHW draw schedule | LOW |
| WaterFixturesMonthlyScheduleMultipliers | /WaterHeating/extension/WaterFixturesMonthlyScheduleMultipliers | DHW seasonal variation | LOW |
| LabelGasRate (ClothesWasher) | /Appliances/ClothesWasher/LabelGasRate | Gas cost for gas washer | LOW |
| LabelAnnualGasCost (ClothesWasher) | /Appliances/ClothesWasher/LabelAnnualGasCost | Annual gas cost | LOW |
| LabelElectricRate (ClothesWasher) | /Appliances/ClothesWasher/LabelElectricRate | Electric cost for washer | LOW |
| LabelGasRate (Dishwasher) | /Appliances/Dishwasher/LabelGasRate | Gas cost for gas dishwasher | LOW |
| LabelAnnualGasCost (Dishwasher) | /Appliances/Dishwasher/LabelAnnualGasCost | Annual gas cost | LOW |
| LabelElectricRate (Dishwasher) | /Appliances/Dishwasher/LabelElectricRate | Electric cost for dishwasher | LOW |
| AdjustedAnnualkWh (Refrigerator) | /Appliances/Refrigerator/extension/AdjustedAnnualkWh | Energy use adjustment | LOW |
| LightingGroup/Load/Units | /Lighting/LightingGroup/Load/Units | Lighting load units (explicit storage) | LOW |
| LightingGroup/Load/Value | /Lighting/LightingGroup/Load/Value | Lighting load value (explicit) | LOW |
| InteriorUsageMultiplier | /Lighting/extension/InteriorUsageMultiplier | Lighting usage multiplier | LOW |
| ExteriorUsageMultiplier | /Lighting/extension/ExteriorUsageMultiplier | Exterior lighting multiplier | LOW |
| GarageUsageMultiplier | /Lighting/extension/GarageUsageMultiplier | Garage lighting multiplier | LOW |
| InteriorWeekdayScheduleFractions | /Lighting/extension/InteriorWeekdayScheduleFractions | Interior lighting schedule | LOW |
| InteriorWeekendScheduleFractions | /Lighting/extension/InteriorWeekendScheduleFractions | Interior lighting schedule | LOW |
| InteriorMonthlyScheduleMultipliers | /Lighting/extension/InteriorMonthlyScheduleMultipliers | Interior lighting seasonal | LOW |
| ExteriorWeekdayScheduleFractions | /Lighting/extension/ExteriorWeekdayScheduleFractions | Exterior lighting schedule | LOW |
| ExteriorWeekendScheduleFractions | /Lighting/extension/ExteriorWeekendScheduleFractions | Exterior lighting schedule | LOW |
| ExteriorMonthlyScheduleMultipliers | /Lighting/extension/ExteriorMonthlyScheduleMultipliers | Exterior lighting seasonal | LOW |
| GarageWeekdayScheduleFractions | /Lighting/extension/GarageWeekdayScheduleFractions | Garage lighting schedule | LOW |
| GarageWeekendScheduleFractions | /Lighting/extension/GarageWeekendScheduleFractions | Garage lighting schedule | LOW |
| GarageMonthlyScheduleMultipliers | /Lighting/extension/GarageMonthlyScheduleMultipliers | Garage lighting seasonal | LOW |
| PlugLoad/extension/MonthlyScheduleMultipliers | /MiscLoads/PlugLoad/extension/MonthlyScheduleMultipliers | Plug load seasonal variation | LOW |
| FuelLoad/extension/MonthlyScheduleMultipliers | /MiscLoads/FuelLoad/extension/MonthlyScheduleMultipliers | Fuel load seasonal variation | LOW |
| Pool/Pumps/Type | /Pools/Pool/Pumps/Type | Pool pump type | LOW |
| Pool/Pumps/Load/Units | /Pools/Pool/Pumps/Load/Units | Pool pump load units | LOW |
| Pool/Heater/Type | /Pools/Pool/Heater/Type | Pool heater type | LOW |
| Pool/Heater/Load/Units | /Pools/Pool/Heater/Load/Units | Pool heater load units | LOW |
| Spa/Pumps/Type | /Spas/Spa/Pumps/Type | Spa pump type | LOW |
| Spa/Pumps/Load/Units | /Spas/Spa/Pumps/Load/Units | Spa pump load units | LOW |
| Spa/Heater/Type | /Spas/Spa/Heater/Type | Spa heater type | LOW |
| Spa/Heater/Load/Units | /Spas/Spa/Heater/Load/Units | Spa heater load units | LOW |

---

## Summary

- **Total missing attributes**: ~50
- **Critical (HIGH)**: 8 attributes that directly affect equipment operation
- **Important (MEDIUM)**: 8 attributes that affect simulation accuracy  
- **Nice-to-Have (LOW)**: ~34 attributes, mostly for detailed scheduling

### Priority Implementation Order

1. **Phase 1 - Critical Fixes**: Weather station, DSE, HeatPumpType/Fuel for standalone systems, JacketRValue
2. **Phase 2 - Important Additions**: NumberofBathrooms, FractionDHWLoadServed, SensibleHeatFraction for heating, appliance identifiers, Pool/Spa type
3. **Phase 3 - Nice-to-Have**: Schedule multipliers, pool/spa details, lighting extensions

---

## Notes

- **WeatherStation/Name**: OCHRE uses this to look up weather data files; without this, HARES relies on external configuration
- **DistributionSystemEfficiency (DSE)**: OCHRE uses this for ducted HVAC systems; HARES calculates DSE from duct parameters but can also accept direct override
- **HeatPumpType in standalone systems**: Identifies heat pumps that are not modeled as combined HeatPump equipment - these may be miss-categorized in HARES
- **JacketRValue**: OCHRE uses this for water heater standby loss calculations
- **FurnitureMass**: Already implemented in HARES (not a gap)
- **FuelLoad/FuelLoadType**: Already implemented in HARES (not a gap) - HARES infers fuel type from load type
- **PlugLoad/FuelLoad Load/Units**: Functionally handled (used for parsing) but not stored as separate config field
