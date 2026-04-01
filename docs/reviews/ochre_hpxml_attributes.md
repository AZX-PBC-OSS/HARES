# OCHRE HPXML Attributes Read

This document lists ALL HPXML attributes that OCHRE parses in `vendors/OCHRE/ochre/utils/hpxml.py`. This is the canonical list used to verify parity with other HPXML parsers.

---

## Building Summary

### Site
- `Site/Latitude` (used indirectly via envelope calculations)
- `Site/Longitude` (used indirectly via envelope calculations)
- `Site/Elevation` (used indirectly via envelope calculations)

### BuildingConstruction
- `BuildingConstruction/ConditionedFloorArea`
- `BuildingConstruction/ConditionedBuildingVolume`
- `BuildingConstruction/AverageCeilingHeight` (optional)
- `BuildingConstruction/NumberofBedrooms`
- `BuildingConstruction/NumberofBathrooms`
- `BuildingConstruction/NumberofConditionedFloors`
- `BuildingConstruction/NumberofConditionedFloorsAboveGrade`
- `BuildingConstruction/ResidentialFacilityType`

### BuildingOccupancy
- `BuildingOccupancy/NumberofResidents`
- `BuildingOccupancy/extension/WeekdayScheduleFractions`
- `BuildingOccupancy/extension/WeekendScheduleFractions`
- `BuildingOccupancy/extension/MonthlyScheduleMultipliers`

---

## Enclosure

### AirInfiltration
- `AirInfiltration/AirInfiltrationMeasurement/BuildingAirLeakage/AirLeakage` (ACH50)
- `AirInfiltration/extension/HasFlueOrChimneyInConditionedSpace`

### Attics
- `Attics/*/AtticType/Attic/Vented`
- `Attics/*/VentilationRate/UnitofMeasure`
- `Attics/*/VentilationRate/Value`

### Foundations
- `Foundations/*/FoundationType` (Crawlspace/Basement)
- `Foundations/*/FoundationType/Crawlspace/Vented`
- `Foundations/*/VentilationRate/UnitofMeasure`
- `Foundations/*/VentilationRate/Value`

### Walls
- `Walls/*/SystemIdentifier/@id`
- `Walls/*/Area`
- `Walls/*/Azimuth`
- `Walls/*/InteriorAdjacentTo`
- `Walls/*/ExteriorAdjacentTo`
- `Walls/*/AttachedToWall/@idref`
- `Walls/*/Insulation/AssemblyEffectiveRValue`
- `Walls/*/Insulation/RValue`
- `Walls/*/SolarAbsorptance`
- `Walls/*/Emittance`
- `Walls/*/Siding`
- `Walls/*/WallType`

### FoundationWalls
- `FoundationWalls/*/SystemIdentifier/@id`
- `FoundationWalls/*/Area`
- `FoundationWalls/*/Height`
- `FoundationWalls/*/InteriorAdjacentTo`
- `FoundationWalls/*/ExteriorAdjacentTo`
- `FoundationWalls/*/Insulation/AssemblyEffectiveRValue`

### Floors (FrameFloors)
- `Floors/*/SystemIdentifier/@id`
- `Floors/*/Area`
- `Floors/*/Azimuth`
- `Floors/*/InteriorAdjacentTo`
- `Floors/*/ExteriorAdjacentTo`
- `Floors/*/FloorOrCeiling`
- `Floors/*/Insulation/AssemblyEffectiveRValue`
- `Floors/*/Insulation/RValue`

### FrameFloors
- Same as Floors

### Slabs
- `Slabs/*/SystemIdentifier/@id`
- `Slabs/*/Area`
- `Slabs/*/InteriorAdjacentTo`
- `Slabs/*/ExteriorAdjacentTo`
- `Slabs/*/Insulation/AssemblyEffectiveRValue`
- `Slabs/*/Insulation/PipeInsulation/PipeRValue` (for slab-on-grade insulation details)

### Roofs
- `Roofs/*/SystemIdentifier/@id`
- `Roofs/*/Area`
- `Roofs/*/Azimuth`
- `Roofs/*/InteriorAdjacentTo`
- `Roofs/*/ExteriorAdjacentTo`
- `Roofs/*/RoofType`
- `Roofs/*/Pitch`
- `Roofs/*/RadiantBarrier`
- `Roofs/*/SolarAbsorptance`
- `Roofs/*/Emittance`

### RimJoists
- `RimJoists/*/SystemIdentifier/@id`
- `RimJoists/*/Area`
- `RimJoists/*/Azimuth`
- `RimJoists/*/InteriorAdjacentTo`
- `RimJoists/*/ExteriorAdjacentTo`
- `RimJoists/*/Siding`

### Windows
- `Windows/*/SystemIdentifier/@id`
- `Windows/*/Area`
- `Windows/*/Azimuth`
- `Windows/*/AttachedToWall/@idref`
- `Windows/*/UFactor`
- `Windows/*/SHGC`
- `Windows/*/InteriorShading/SummerShadingCoefficient`

### Doors
- `Doors/*/SystemIdentifier/@id`
- `Doors/*/Area`
- `Doors/*/Azimuth`
- `Doors/*/AttachedToWall/@idref`

### Enclosure Extension
- `Enclosure/extension/PartitionWallMass/AreaFraction`
- `Enclosure/extension/PartitionWallMass/InteriorFinish/Type`
- `Enclosure/extension/PartitionWallMass/InteriorFinish/Thickness`
- `Enclosure/extension/FurnitureMass/AreaFraction`

---

## Systems

### HVAC

#### HVACPlant (Heating)
- `Systems/HVAC/HVACPlant/HeatingSystem/HeatingSystemType`
- `Systems/HVAC/HVACPlant/HeatingSystem/HeatPumpType`
- `Systems/HVAC/HVACPlant/HeatingSystem/HeatPumpFuel`
- `Systems/HVAC/HVACPlant/HeatingSystem/HeatingCapacity`
- `Systems/HVAC/HVACPlant/HeatingSystem/FractionHeatingLoadServed`
- `Systems/HVAC/HVACPlant/HeatingSystem/AnnualHeatingEfficiency/Units`
- `Systems/HVAC/HVACPlant/HeatingSystem/AnnualHeatingEfficiency/Value`
- `Systems/HVAC/HVACPlant/HeatingSystem/CompressorType`
- `Systems/HVAC/HVACPlant/HeatingSystem/SensibleHeatFraction`
- `Systems/HVAC/HVACPlant/HeatingSystem/extension/FanPowerWattsPerCFM`
- `Systems/HVAC/HVACPlant/HeatingSystem/extension/FanPowerWatts`
- `Systems/HVAC/HVACPlant/HeatingSystem/extension/ElectricAuxiliaryEnergy`

#### HVACPlant (Cooling)
- `Systems/HVAC/HVACPlant/CoolingSystem/CoolingSystemType`
- `Systems/HVAC/HVACPlant/CoolingSystem/HeatPumpType`
- `Systems/HVAC/HVACPlant/CoolingSystem/HeatPumpFuel`
- `Systems/HVAC/HVACPlant/CoolingSystem/CoolingCapacity`
- `Systems/HVAC/HVACPlant/CoolingSystem/FractionCoolingLoadServed`
- `Systems/HVAC/HVACPlant/CoolingSystem/AnnualCoolingEfficiency/Units`
- `Systems/HVAC/HVACPlant/CoolingSystem/AnnualCoolingEfficiency/Value`
- `Systems/HVAC/HVACPlant/CoolingSystem/CompressorType`
- `Systems/HVAC/HVACPlant/CoolingSystem/CoolingSensibleHeatFraction`
- `Systems/HVAC/HVACPlant/CoolingSystem/extension/FanPowerWattsPerCFM`
- `Systems/HVAC/HVACPlant/CoolingSystem/extension/FanPowerWatts`
- `Systems/HVAC/HVACPlant/CoolingSystem/extension/ElectricAuxiliaryEnergy`

#### Heat Pump (combined heating/cooling)
- `Systems/HVAC/HVACPlant/HeatPump/HeatPumpType`
- `Systems/HVAC/HVACPlant/HeatPump/HeatPumpFuel`
- `Systems/HVAC/HVACPlant/HeatPump/HeatingCapacity`
- `Systems/HVAC/HVACPlant/HeatPump/CoolingCapacity`
- `Systems/HVAC/HVACPlant/HeatPump/FractionHeatLoadServed`
- `Systems/HVAC/HVACPlant/HeatPump/FractionCoolLoadServed`
- `Systems/HVAC/HVACPlant/HeatPump/AnnualHeatingEfficiency/Units`
- `Systems/HVAC/HVACPlant/HeatPump/AnnualHeatingEfficiency/Value`
- `Systems/HVAC/HVACPlant/HeatPump/AnnualCoolingEfficiency/Units`
- `Systems/HVAC/HVACPlant/HeatPump/AnnualCoolingEfficiency/Value`
- `Systems/HVAC/HVACPlant/HeatPump/CompressorType`
- `Systems/HVAC/HVACPlant/HeatPump/CoolingSensibleHeatFraction`
- `Systems/HVAC/HVACPlant/HeatPump/BackupSystemFuel`
- `Systems/HVAC/HVACPlant/HeatPump/BackupHeatingCapacity`
- `Systems/HVAC/HVACPlant/HeatPump/BackupAnnualHeatingEfficiency/Value`
- `Systems/HVAC/HVACPlant/HeatPump/CompressorLockoutTemperature`
- `Systems/HVAC/HVACPlant/HeatPump/BackupHeatingSwitchoverTemperature`
- `Systems/HVAC/HVACPlant/HeatPump/BackupHeatingLockoutTemperature`
- `Systems/HVAC/HVACPlant/HeatPump/extension/FanPowerWattsPerCFM`
- `Systems/HVAC/HVACPlant/HeatPump/extension/FanPowerWatts`
- `Systems/HVAC/HVACPlant/HeatPump/extension/ElectricAuxiliaryEnergy`

#### HVACControl
- `Systems/HVAC/HVACControl/SetpointTempHeatingSeason`
- `Systems/HVAC/HVACControl/SetpointTempCoolingSeason`
- `Systems/HVAC/HVACControl/extension/WeekdaySetpointTempsHeatingSeason`
- `Systems/HVAC/HVACControl/extension/WeekendSetpointTempsHeatingSeason`
- `Systems/HVAC/HVACControl/extension/WeekdaySetpointTempsCoolingSeason`
- `Systems/HVAC/HVACControl/extension/WeekendSetpointTempsCoolingSeason`
- `Systems/HVAC/HVACControl/extension/MonthlyScheduleMultipliers`

#### HVACDistribution
- `Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/DuctLeakageMeasurement`
- `Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/*/DuctLocation`
- `Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/*/DuctType`
- `Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/*/DuctSurfaceArea`
- `Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/*/DuctInsulationRValue`
- `Systems/HVAC/HVACDistribution/AnnualHeatingDistributionSystemEfficiency`
- `Systems/HVAC/HVACDistribution/AnnualCoolingDistributionSystemEfficiency`

### WaterHeating

#### WaterHeatingSystem
- `Systems/WaterHeating/WaterHeatingSystem/WaterHeaterType`
- `Systems/WaterHeating/WaterHeatingSystem/FuelType`
- `Systems/WaterHeating/WaterHeatingSystem/Location`
- `Systems/WaterHeating/WaterHeatingSystem/HotWaterTemperature`
- `Systems/WaterHeating/WaterHeatingSystem/EnergyFactor`
- `Systems/WaterHeating/WaterHeatingSystem/UniformEnergyFactor`
- `Systems/WaterHeating/WaterHeatingSystem/TankVolume`
- `Systems/WaterHeating/WaterHeatingSystem/TankHeight`
- `Systems/WaterHeating/WaterHeatingSystem/HeatingCapacity`
- `Systems/WaterHeating/WaterHeatingSystem/FirstHourRating`
- `Systems/WaterHeating/WaterHeatingSystem/RecoveryEfficiency`
- `Systems/WaterHeating/WaterHeatingSystem/WaterHeaterInsulation/Jacket/JacketRValue`
- `Systems/WaterHeating/WaterHeatingSystem/FractionDHWLoadServed`
- `Systems/WaterHeating/WaterHeatingSystem/PerformanceAdjustment`

#### WaterFixture
- `Systems/WaterHeating/WaterFixture/LowFlow`

#### HotWaterDistribution
- `Systems/WaterHeating/HotWaterDistribution/SystemType/Standard/PipingLength`
- `Systems/WaterHeating/HotWaterDistribution/SystemType/Recirculation/BranchPipingLoopLength`
- `Systems/WaterHeating/HotWaterDistribution/PipeInsulation/PipeRValue`

#### WaterHeating Extension
- `Systems/WaterHeating/extension/WaterFixturesUsageMultiplier`
- `Systems/WaterHeating/extension/WaterFixturesWeekdayScheduleFractions`
- `Systems/WaterHeating/extension/WaterFixturesWeekendScheduleFractions`
- `Systems/WaterHeating/extension/WaterFixturesMonthlyScheduleMultipliers`

### MechanicalVentilation

#### VentilationFans
- `Systems/MechanicalVentilation/VentilationFans/*/FanType`
- `Systems/MechanicalVentilation/VentilationFans/*/RatedFlowRate`
- `Systems/MechanicalVentilation/VentilationFans/*/FanPower`
- `Systems/MechanicalVentilation/VentilationFans/*/UsedForWholeBuildingVentilation`
- `Systems/MechanicalVentilation/VentilationFans/*/UsedForSeasonalCoolingLoadReduction`
- `Systems/MechanicalVentilation/VentilationFans/*/HoursInOperation`
- `Systems/MechanicalVentilation/VentilationFans/*/SensibleRecoveryEfficiency`
- `Systems/MechanicalVentilation/VentilationFans/*/TotalRecoveryEfficiency`

---

## Appliances

### ClothesWasher
- `Appliances/ClothesWasher/RatedAnnualkWh`
- `Appliances/ClothesWasher/Capacity`
- `Appliances/ClothesWasher/LabelUsage`
- `Appliances/ClothesWasher/LabelGasRate`
- `Appliances/ClothesWasher/LabelAnnualGasCost`
- `Appliances/ClothesWasher/LabelElectricRate`
- `Appliances/ClothesWasher/extension/UsageMultiplier`

### ClothesDryer
- `Appliances/ClothesDryer/CombinedEnergyFactor`
- `Appliances/ClothesDryer/EnergyFactor`
- `Appliances/ClothesDryer/FuelType`
- `Appliances/ClothesDryer/Vented`
- `Appliances/ClothesDryer/extension/UsageMultiplier`

### Dishwasher
- `Appliances/Dishwasher/RatedAnnualkWh`
- `Appliances/Dishwasher/PlaceSettingCapacity`
- `Appliances/Dishwasher/LabelUsage`
- `Appliances/Dishwasher/LabelGasRate`
- `Appliances/Dishwasher/LabelAnnualGasCost`
- `Appliances/Dishwasher/LabelElectricRate`
- `Appliances/Dishwasher/extension/UsageMultiplier`

### Refrigerator
- `Appliances/Refrigerator/PrimaryIndicator`
- `Appliances/Refrigerator/RatedAnnualkWh`
- `Appliances/Refrigerator/extension/AdjustedAnnualkWh`
- `Appliances/Refrigerator/extension/UsageMultiplier`

### Freezer
- `Appliances/Freezer/RatedAnnualkWh`
- `Appliances/Freezer/extension/UsageMultiplier`

### CookingRange
- `Appliances/CookingRange/FuelType`
- `Appliances/CookingRange/IsInduction`
- `Appliances/CookingRange/Location`
- `Appliances/CookingRange/extension/UsageMultiplier`

### Oven
- `Appliances/Oven/IsConvection`

---

## Lighting

### LightingGroup
- `Lighting/LightingGroup/Location` (interior/exterior/garage)
- `Lighting/LightingGroup/LightingType` (CompactFluorescent/FluorescentTube/LightEmittingDiode)
- `Lighting/LightingGroup/FractionofUnitsInLocation`
- `Lighting/LightingGroup/Load/Units`
- `Lighting/LightingGroup/Load/Value`

### CeilingFan
- `Lighting/CeilingFan/Count`
- `Lighting/CeilingFan/Airflow/Efficiency`

### Lighting Extension
- `Lighting/extension/InteriorUsageMultiplier`
- `Lighting/extension/ExteriorUsageMultiplier`
- `Lighting/extension/GarageUsageMultiplier`
- `Lighting/extension/InteriorWeekdayScheduleFractions`
- `Lighting/extension/InteriorWeekendScheduleFractions`
- `Lighting/extension/InteriorMonthlyScheduleMultipliers`
- `Lighting/extension/ExteriorWeekdayScheduleFractions`
- `Lighting/extension/ExteriorWeekendScheduleFractions`
- `Lighting/extension/ExteriorMonthlyScheduleMultipliers`
- `Lighting/extension/GarageWeekdayScheduleFractions`
- `Lighting/extension/GarageWeekendScheduleFractions`
- `Lighting/extension/GarageMonthlyScheduleMultipliers`

---

## Miscellaneous Loads (MELs/MGLs)

### PlugLoad (MELs)
- `MiscLoads/PlugLoad/PlugLoadType` (TV other/other/well pump/electric vehicle charging/etc.)
- `MiscLoads/PlugLoad/Load/Units`
- `MiscLoads/PlugLoad/Load/Value`
- `MiscLoads/PlugLoad/extension/UsageMultiplier`
- `MiscLoads/PlugLoad/extension/FracSensible`
- `MiscLoads/PlugLoad/extension/FracLatent`
- `MiscLoads/PlugLoad/extension/WeekdayScheduleFractions`
- `MiscLoads/PlugLoad/extension/WeekendScheduleFractions`
- `MiscLoads/PlugLoad/extension/MonthlyScheduleMultipliers`

### FuelLoad (MGLs)
- `MiscLoads/FuelLoad/FuelLoadType` (grill/fireplace/lighting)
- `MiscLoads/FuelLoad/FuelType`
- `MiscLoads/FuelLoad/Load/Units`
- `MiscLoads/FuelLoad/Load/Value`
- `MiscLoads/FuelLoad/extension/UsageMultiplier`
- `MiscLoads/FuelLoad/extension/FracSensible`
- `MiscLoads/FuelLoad/extension/FracLatent`
- `MiscLoads/FuelLoad/extension/WeekdayScheduleFractions`
- `MiscLoads/FuelLoad/extension/WeekendScheduleFractions`
- `MiscLoads/FuelLoad/extension/MonthlyScheduleMultipliers`

---

## Pool/Spa Equipment

### Pools
- `Pools/Pool/Type`
- `Pools/Pool/Pumps/Type`
- `Pools/Pool/Pumps/Load/Units`
- `Pools/Pool/Pumps/Load/Value`
- `Pools/Pool/Heater/Type`
- `Pools/Pool/Heater/Load/Units`
- `Pools/Pool/Heater/Load/Value`

### Spas
- `Spas/Spa/Type`
- `Spas/Spa/Pumps/Type`
- `Spas/Spa/Pumps/Load/Units`
- `Spas/Spa/Pumps/Load/Value`
- `Spas/Spa/Heater/Type`
- `Spas/Spa/Heater/Load/Units`
- `Spas/Spa/Heater/Load/Value`

---

## Climate and Risk Zones

### WeatherStation
- `ClimateandRiskZones/WeatherStation/Name`

---

## Summary Statistics

- **Total unique HPXML elements read**: ~150+
- **Main sections**: BuildingSummary, Enclosure, Systems (HVAC, WaterHeating, MechanicalVentilation), Appliances, Lighting, MiscLoads, Pools/Spas, ClimateandRiskZones
- **File location**: `vendors/OCHRE/ochre/utils/hpxml.py`

---

## Notes

1. OCHRE uses `xmltodict` to parse HPXML files into Python dictionaries
2. All access patterns use dictionary `.get()` and direct `[]` access
3. The HPXML is pre-processed by `convert_hpxml_element()` in `base.py` which:
   - Converts list elements to dictionaries keyed by SystemIdentifier/@id
   - Removes redundant @id information
   - Converts string "true"/"false" to boolean
   - Evaluates strings to floats/lists where possible
4. Some attributes have default values if not present in HPXML
5. Some attributes are optional and may be None if not specified
