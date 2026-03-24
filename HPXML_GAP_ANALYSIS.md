# HPXML Gap Analysis: OCHRE vs HARES

This document compares HPXML properties that OCHRE reads vs what HARES currently implements.

## Summary Statistics
- **OCHRE**: 200+ HPXML elements across 24 categories
- **HARES**: ~120 HPXML elements across 15 categories
- **Gap**: ~80 elements not yet implemented in HARES

---

## Detailed Gap Analysis by Category

### 1. Building Summary

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `ResidentialFacilityType` (house type) | ✅ | ❌ | **MISSING** |
| `NumberofConditionedFloors` (total) | ✅ | Partial | Need total vs above-grade distinction |
| `NumberofConditionedFloorsAboveGrade` | ✅ | ✅ | Implemented |
| `NumberofBedrooms` | ✅ | ✅ | Implemented |
| `NumberofBathrooms` | ✅ | ❌ | **MISSING** |
| `ConditionedFloorArea` | ✅ | ✅ | Implemented |
| `ConditionedBuildingVolume` | ✅ | ✅ | Implemented |
| `AverageCeilingHeight` | ✅ | Derived | Calculated from volume/area |
| `NumberofResidents` | ✅ | ✅ | Implemented |
| `Site/SiteType` | ✅ | ✅ | Implemented |
| `Site/ShieldingOfHome` | ✅ | ✅ | Implemented |
| `Site/Elevation` | ✅ | ✅ | Implemented |
| `Latitude`, `Longitude` | ✅ | ✅ | Implemented |

**Gap**: House type, total floor count, bathrooms

---

### 2. Enclosure - Air Infiltration

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `HousePressure` | ✅ | ❌ | **MISSING** |
| `UnitofMeasure` (CFM50, ACH50, etc) | ✅ | Partial | Only ACH50 handled |
| `AirLeakage` value | ✅ | ✅ | Implemented |
| `InfiltrationVolume` | ✅ | ✅ | Implemented |
| `InfiltrationHeight` | ✅ | ✅ | Implemented |
| `HasFlueOrChimneyInConditionedSpace` | ✅ | ✅ | Implemented |

**Gap**: House pressure for infiltration calculations, other unit types (CFM50, etc)

---

### 3. Enclosure - Attics

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `AtticType/Attic/Vented` | ✅ | ✅ | Zone.vented field |
| `VentilationRate/UnitofMeasure` | ✅ | Partial | Only ACH handled |
| `VentilationRate/Value` | ✅ | ✅ | Implemented |
| `AttachedToRoof` | ✅ | ❌ | **MISSING** - Links not resolved |
| `AttachedToWall` | ✅ | ❌ | **MISSING** - Links not resolved |
| `AttachedToFloor` | ✅ | ❌ | **MISSING** - Links not resolved |

**Gap**: HPXML ID reference resolution for attic-roof-wall-floor connections

---

### 4. Enclosure - Foundations

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `FoundationType` | ✅ | Partial | Basic enum only |
| `FoundationType/Basement/Conditioned` | ✅ | ❌ | **MISSING** - Finished vs unfinished |
| `FoundationType/Crawlspace/Vented` | ✅ | ✅ | Zone.vented field |
| `VentilationRate/*` | ✅ | Partial | Only ACH handled |
| `AttachedToRimJoist` | ✅ | ❌ | **MISSING** - Links |
| `AttachedToFoundationWall` | ✅ | ❌ | **MISSING** - Links |
| `AttachedToSlab` | ✅ | ❌ | **MISSING** - Links |

**Gap**: Foundation conditioning status, HPXML ID reference resolution

---

### 5. Enclosure - Roofs

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `InteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `ExteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `Area` | ✅ | ✅ | Implemented |
| `RoofType` (finish) | ✅ | ✅ | Implemented |
| `SolarAbsorptance` | ✅ | ✅ | Implemented |
| `Emittance` | ✅ | ✅ | Implemented |
| `Pitch` (converted to tilt) | ✅ | ✅ | Implemented |
| `RadiantBarrier` | ✅ | ✅ | Implemented |
| `Insulation/AssemblyEffectiveRValue` | ✅ | ✅ | Implemented |

**Status**: ✅ Complete

---

### 6. Enclosure - Walls

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `ExteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `InteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `WallType` (construction) | ✅ | ✅ | construction_type field |
| `Area` | ✅ | ✅ | Implemented |
| `Siding` (finish) | ✅ | ✅ | finish_type field |
| `SolarAbsorptance` | ✅ | ✅ | Implemented |
| `Emittance` | ✅ | ✅ | Implemented |
| `AtticWallType` | ✅ | ❌ | **MISSING** - Gable walls |
| `InteriorFinish/Type` | ✅ | ❌ | **MISSING** |
| `Insulation/AssemblyEffectiveRValue` | ✅ | ✅ | Implemented |

**Gap**: Attic wall type (gable walls), interior finish details

---

### 7. Enclosure - Foundation Walls

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `ExteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `InteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `Height` | ✅ | ✅ | Implemented |
| `Area` | ✅ | ✅ | Implemented |
| `Thickness` | ✅ | ❌ | **MISSING** |
| `DepthBelowGrade` | ✅ | ❌ | **MISSING** |
| `InteriorFinish/Type` | ✅ | ❌ | **MISSING** |
| `Insulation/Layer/InstallationType` | ✅ | ❌ | **MISSING** |
| `Insulation/Layer/NominalRValue` | ✅ | ✅ | Implemented |
| `Insulation/Layer/DistanceToTopOfInsulation` | ✅ | ❌ | **MISSING** |
| `Insulation/Layer/DistanceToBottomOfInsulation` | ✅ | ❌ | **MISSING** |

**Gap**: Foundation wall thickness, depth below grade, detailed insulation positioning

---

### 8. Enclosure - Floors/FrameFloors

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `ExteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `InteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `FloorOrCeiling` | ✅ | ❌ | **MISSING** |
| `FloorType` (construction) | ✅ | ❌ | **MISSING** |
| `Area` | ✅ | ✅ | Implemented |
| `InteriorFinish/Type` | ✅ | ❌ | **MISSING** |
| `Insulation/AssemblyEffectiveRValue` | ✅ | ✅ | Implemented |

**Gap**: Floor/ceiling designation, construction type, interior finish

---

### 9. Enclosure - Slabs

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `InteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `Area` | ✅ | ✅ | Implemented |
| `Thickness` | ✅ | ✅ | Implemented |
| `ExposedPerimeter` | ✅ | ✅ | Implemented |
| `PerimeterInsulation/Layer/NominalRValue` | ✅ | ✅ | Implemented |
| `PerimeterInsulation/Layer/InsulationDepth` | ✅ | ❌ | **MISSING** |
| `UnderSlabInsulation/Layer/NominalRValue` | ✅ | ❌ | **MISSING** |
| `UnderSlabInsulation/Layer/InsulationWidth` | ✅ | ❌ | **MISSING** |
| `extension/CarpetFraction` | ✅ | ❌ | **MISSING** |
| `extension/CarpetRValue` | ✅ | ❌ | **MISSING** |

**Gap**: Under-slab insulation, carpet properties

---

### 10. Enclosure - Rim Joists

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `ExteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `InteriorAdjacentTo` | ✅ | ✅ | Zone mapping |
| `Area` | ✅ | ✅ | Implemented |
| `Siding` (finish) | ✅ | ❌ | **MISSING** |
| `SolarAbsorptance` | ✅ | ✅ | Implemented |
| `Emittance` | ✅ | ✅ | Implemented |
| `Insulation/AssemblyEffectiveRValue` | ✅ | ✅ | Implemented |

**Gap**: Rim joist finish type

---

### 11. Enclosure - Windows

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Area` | ✅ | ✅ | Implemented |
| `Azimuth` | ✅ | ✅ | Implemented |
| `UFactor` | ✅ | ✅ | Implemented |
| `SHGC` | ✅ | ✅ | Implemented |
| `InteriorShading/SummerShadingCoefficient` | ✅ | ✅ | interior_shading_fraction |
| `InteriorShading/WinterShadingCoefficient` | ✅ | ❌ | **MISSING** |
| `FractionOperable` | ✅ | ❌ | **MISSING** |
| `AttachedToWall` | ✅ | ✅ | attached_to_wall_id |
| `FrameType` | ✅ | ✅ | Implemented |

**Gap**: Winter shading coefficient, operable fraction

---

### 12. Enclosure - Doors

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Area` | ✅ | ✅ | Implemented |
| `Azimuth` | ✅ | ✅ | Implemented |
| `RValue` | ✅ | ✅ | Implemented |
| `AttachedToWall` | ✅ | ✅ | Implemented |

**Status**: ✅ Complete

---

### 13. Enclosure - Extension Elements

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `PartitionWallMass/AreaFraction` | ✅ | ❌ | **MISSING** |
| `PartitionWallMass/InteriorFinish/Type` | ✅ | ❌ | **MISSING** |
| `PartitionWallMass/InteriorFinish/Thickness` | ✅ | ❌ | **MISSING** |
| `FurnitureMass/AreaFraction` | ✅ | ❌ | **MISSING** |

**Gap**: Internal mass modeling (partition walls, furniture)

---

### 14. HVAC - Heating Systems

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `HeatingSystemType/*` (detailed) | ✅ | Partial | Basic types only |
| `HeatingSystemFuel` | ✅ | ✅ | Implemented |
| `HeatingCapacity` | ✅ | ✅ | Implemented |
| `FractionHeatLoadServed` | ✅ | ✅ | Implemented |
| `AnnualHeatingEfficiency` | ✅ | ✅ | Implemented |
| `ElectricAuxiliaryEnergy` | ✅ | ✅ | Implemented |
| `extension/FanPowerWattsPerCFM` | ✅ | ✅ | Implemented |
| `extension/FanPowerWatts` | ✅ | ✅ | Implemented |
| `extension/HeatingAirflowCFM` | ✅ | ❌ | **MISSING** |

**Gap**: Detailed furnace types (wall, floor), heating airflow CFM

---

### 15. HVAC - Cooling Systems

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `CoolingSystemType` | ✅ | ✅ | Implemented |
| `CoolingCapacity` | ✅ | ✅ | Implemented |
| `FractionCoolLoadServed` | ✅ | ✅ | Implemented |
| `AnnualCoolingEfficiency` | ✅ | ✅ | Implemented |
| `CompressorType` | ✅ | ✅ | Implemented |
| `SensibleHeatFraction` | ✅ | ✅ | Implemented |
| `extension/FanPowerWattsPerCFM` | ✅ | ✅ | Implemented |
| `extension/FanPowerWatts` | ✅ | ✅ | Implemented |
| `extension/CoolingAirflowCFM` | ✅ | ❌ | **MISSING** |

**Gap**: Cooling airflow CFM

---

### 16. HVAC - Heat Pumps

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `HeatPumpType` | ✅ | ✅ | Implemented |
| `HeatPumpFuel` | ✅ | ✅ | Implemented |
| `HeatingCapacity` | ✅ | ✅ | Implemented |
| `CoolingCapacity` | ✅ | ✅ | Implemented |
| `AnnualHeatingEfficiency` | ✅ | ✅ | Implemented |
| `AnnualCoolingEfficiency` | ✅ | ✅ | Implemented |
| `CoolingSensibleHeatFraction` | ✅ | ✅ | Implemented |
| `BackupSystemFuel` | ✅ | ✅ | Implemented |
| `BackupHeatingCapacity` | ✅ | ✅ | Implemented |
| `BackupAnnualHeatingEfficiency` | ✅ | ✅ | Implemented |
| `CompressorLockoutTemperature` | ✅ | ✅ | Implemented |
| `BackupHeatingLockoutTemperature` | ✅ | ✅ | Implemented |
| `BackupHeatingSwitchoverTemperature` | ✅ | ✅ | Implemented |
| `FractionHeatingLoadServed` | ✅ | ✅ | Implemented |
| `FractionCoolingLoadServed` | ✅ | ✅ | Implemented |

**Status**: ✅ Complete

---

### 17. HVAC - Controls

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `SetpointTempHeatingSeason` | ✅ | ✅ | Implemented |
| `SetpointTempCoolingSeason` | ✅ | ✅ | Implemented |
| `extension/WeekdaySetpointTempsHeatingSeason` | ✅ | ✅ | Implemented |
| `extension/WeekendSetpointTempsHeatingSeason` | ✅ | ✅ | Implemented |
| `extension/WeekdaySetpointTempsCoolingSeason` | ✅ | ✅ | Implemented |
| `extension/WeekendSetpointTempsCoolingSeason` | ✅ | ✅ | Implemented |

**Status**: ✅ Complete

---

### 18. HVAC - Distribution

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `AirDistribution` | ✅ | ✅ | Implemented |
| `AnnualHeatingDistributionSystemEfficiency` | ✅ | ❌ | **MISSING** - DSE |
| `AnnualCoolingDistributionSystemEfficiency` | ✅ | ❌ | **MISSING** - DSE |
| `DuctLeakageMeasurement` | ✅ | Partial | Leakage fraction only |
| `Ducts/DuctType` | ✅ | ✅ | Implemented |
| `Ducts/DuctLocation` | ✅ | ✅ | Implemented |
| `Ducts/DuctInsulationRValue` | ✅ | ✅ | Implemented |
| `Ducts/DuctSurfaceArea` | ✅ | ✅ | Implemented |

**Gap**: DSE (Distribution System Efficiency) from HPXML, detailed duct leakage

---

### 19. Mechanical Ventilation

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `UsedForWholeBuildingVentilation` | ✅ | ✅ | Implemented |
| `UsedForSeasonalCoolingLoadReduction` | ✅ | ✅ | Whole house fan |
| `FanType` | ✅ | ✅ | Implemented |
| `RatedFlowRate` | ✅ | ✅ | Implemented |
| `SensibleRecoveryEfficiency` | ✅ | ✅ | Implemented |
| `TotalRecoveryEfficiency` | ✅ | ✅ | Implemented |
| `HoursInOperation` | ✅ | ❌ | **MISSING** |
| `FanPower` | ✅ | ✅ | Implemented |

**Gap**: Hours validation (OCHRE requires 24 hours)

---

### 20. Water Heating

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `WaterHeaterType` | ✅ | ✅ | Implemented |
| `FuelType` | ✅ | ✅ | Implemented |
| `Location` | ✅ | ✅ | Implemented |
| `TankVolume` | ✅ | ✅ | Implemented |
| `TankHeight` | ✅ | ❌ | **MISSING** |
| `HeatingCapacity` | ✅ | ✅ | Implemented |
| `FractionDHWLoadServed` | ✅ | ❌ | **MISSING** |
| `EnergyFactor` | ✅ | ✅ | Implemented |
| `UniformEnergyFactor` | ✅ | ✅ | Implemented |
| `RecoveryEfficiency` | ✅ | ✅ | Implemented |
| `FirstHourRating` | ✅ | ✅ | Implemented |
| `HotWaterTemperature` | ✅ | ✅ | Implemented |
| `PerformanceAdjustment` | ✅ | ✅ | Implemented |
| `WaterHeaterInsulation/Jacket/JacketRValue` | ✅ | ✅ | Implemented |
| `HotWaterDistribution` | ✅ | ✅ | Implemented |
| `WaterFixture/WaterFixtureType` | ✅ | ❌ | **MISSING** |
| `WaterFixture/LowFlow` | ✅ | ✅ | Implemented |
| `extension/WaterFixturesUsageMultiplier` | ✅ | ✅ | Implemented |
| `extension/WaterFixturesWeekdayScheduleFractions` | ✅ | ❌ | **MISSING** |
| `extension/WaterFixturesWeekendScheduleFractions` | ✅ | ❌ | **MISSING** |
| `extension/WaterFixturesMonthlyScheduleMultipliers` | ✅ | ❌ | **MISSING** |

**Gap**: Tank height, fraction DHW load served, water fixture types, fixture schedules

---

### 21. Appliances - Clothes Washer

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Location` | ✅ | ❌ | **MISSING** |
| `IntegratedModifiedEnergyFactor` | ✅ | ✅ | Implemented |
| `RatedAnnualkWh` | ✅ | ✅ | Implemented |
| `LabelElectricRate` | ✅ | ❌ | **MISSING** |
| `LabelGasRate` | ✅ | ❌ | **MISSING** |
| `LabelAnnualGasCost` | ✅ | ❌ | **MISSING** |
| `LabelUsage` | ✅ | ✅ | Implemented |
| `Capacity` | ✅ | ✅ | Implemented |

**Gap**: Location, label rates/costs (for OCHRE rating calculations)

---

### 22. Appliances - Clothes Dryer

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Location` | ✅ | ❌ | **MISSING** |
| `FuelType` | ✅ | ✅ | Implemented |
| `CombinedEnergyFactor` | ✅ | ✅ | Implemented |
| `EnergyFactor` | ✅ | ✅ | Implemented |
| `Vented` | ✅ | ✅ | Implemented |
| `VentedFlowRate` | ✅ | ❌ | **MISSING** |
| `extension/UsageMultiplier` | ✅ | ✅ | Implemented |
| `extension/WeekdayScheduleFractions` | ✅ | ✅ | Implemented |
| `extension/WeekendScheduleFractions` | ✅ | ✅ | Implemented |
| `extension/MonthlyScheduleMultipliers` | ✅ | ✅ | Implemented |

**Gap**: Location, vented flow rate

---

### 23. Appliances - Dishwasher

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Location` | ✅ | ❌ | **MISSING** |
| `RatedAnnualkWh` | ✅ | ✅ | Implemented |
| `PlaceSettingCapacity` | ✅ | ✅ | Implemented |
| `LabelElectricRate` | ✅ | ❌ | **MISSING** |
| `LabelGasRate` | ✅ | ❌ | **MISSING** |
| `LabelAnnualGasCost` | ✅ | ❌ | **MISSING** |
| `LabelUsage` | ✅ | ✅ | Implemented |
| `extension/UsageMultiplier` | ✅ | ✅ | Implemented |
| `extension/WeekdayScheduleFractions` | ✅ | ✅ | Implemented |
| `extension/WeekendScheduleFractions` | ✅ | ✅ | Implemented |
| `extension/MonthlyScheduleMultipliers` | ✅ | ✅ | Implemented |

**Gap**: Location, label rates/costs

---

### 24. Appliances - Refrigerator

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Location` | ✅ | ❌ | **MISSING** |
| `RatedAnnualkWh` | ✅ | ✅ | Implemented |
| `PrimaryIndicator` | ✅ | ❌ | **MISSING** |
| `extension/AdjustedAnnualkWh` | ✅ | ❌ | **MISSING** |
| `extension/UsageMultiplier` | ✅ | ✅ | Implemented |
| `extension/WeekdayScheduleFractions` | ✅ | ✅ | Implemented |
| `extension/WeekendScheduleFractions` | ✅ | ✅ | Implemented |
| `extension/MonthlyScheduleMultipliers` | ✅ | ✅ | Implemented |

**Gap**: Location, primary indicator, adjusted kWh

---

### 25. Appliances - Freezer

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Location` | ✅ | ❌ | **MISSING** |
| `RatedAnnualkWh` | ✅ | ✅ | Implemented |
| `extension/UsageMultiplier` | ✅ | ✅ | Implemented |
| `extension/WeekdayScheduleFractions` | ✅ | ✅ | Implemented |
| `extension/WeekendScheduleFractions` | ✅ | ✅ | Implemented |
| `extension/MonthlyScheduleMultipliers` | ✅ | ✅ | Implemented |

**Gap**: Location

---

### 26. Appliances - Cooking Range

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Location` | ✅ | ❌ | **MISSING** |
| `FuelType` | ✅ | ✅ | Implemented |
| `IsInduction` | ✅ | ❌ | **MISSING** |
| `extension/UsageMultiplier` | ✅ | ✅ | Implemented |
| `extension/WeekdayScheduleFractions` | ✅ | ✅ | Implemented |
| `extension/WeekendScheduleFractions` | ✅ | ✅ | Implemented |
| `extension/MonthlyScheduleMultipliers` | ✅ | ✅ | Implemented |

**Gap**: Location, induction flag

---

### 27. Appliances - Oven

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `IsConvection` | ✅ | ❌ | **MISSING** |

**Gap**: Convection oven flag

---

### 28. Lighting

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `LightingGroup/Location` | ✅ | ✅ | Implemented |
| `FractionofUnitsInLocation` | ✅ | ✅ | Implemented |
| `LightingType/CompactFluorescent` | ✅ | ✅ | Implemented |
| `LightingType/FluorescentTube` | ✅ | ✅ | Implemented |
| `LightingType/LightEmittingDiode` | ✅ | ✅ | Implemented |
| `Load/Units` | ✅ | Partial | kWh only |
| `Load/Value` | ✅ | ✅ | Implemented |
| `CeilingFan/Count` | ✅ | ✅ | Implemented |
| `CeilingFan/Airflow/Efficiency` | ✅ | ✅ | Implemented |
| `extension/InteriorUsageMultiplier` | ✅ | ❌ | **MISSING** |
| `extension/ExteriorUsageMultiplier` | ✅ | ❌ | **MISSING** |
| `extension/GarageUsageMultiplier` | ✅ | ❌ | **MISSING** |
| `extension/*WeekdayScheduleFractions` | ✅ | ❌ | **MISSING** |
| `extension/*WeekendScheduleFractions` | ✅ | ❌ | **MISSING** |
| `extension/*MonthlyScheduleMultipliers` | ✅ | Partial | Ceiling fan only |

**Gap**: Lighting usage multipliers, lighting schedule fractions

---

### 29. Misc Loads (MELs/MGLs)

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `PlugLoad/PlugLoadType` | ✅ | ✅ | Implemented |
| `PlugLoad/Load/Units` | ✅ | ✅ | Implemented |
| `PlugLoad/Load/Value` | ✅ | ✅ | Implemented |
| `PlugLoad/extension/UsageMultiplier` | ✅ | ✅ | Implemented |
| `PlugLoad/extension/FracSensible` | ✅ | ✅ | Implemented |
| `PlugLoad/extension/FracLatent` | ✅ | ✅ | Implemented |
| `PlugLoad/extension/WeekdayScheduleFractions` | ✅ | ✅ | Implemented |
| `PlugLoad/extension/WeekendScheduleFractions` | ✅ | ✅ | Implemented |
| `PlugLoad/extension/MonthlyScheduleMultipliers` | ✅ | ✅ | Implemented |
| `FuelLoad/FuelLoadType` | ✅ | ✅ | Implemented |
| `FuelLoad/FuelType` | ✅ | ✅ | Implemented |
| `FuelLoad/Load/Units` | ✅ | ✅ | Implemented |
| `FuelLoad/Load/Value` | ✅ | ✅ | Implemented |
| `FuelLoad/extension/UsageMultiplier` | ✅ | ✅ | Implemented |

**Status**: ✅ Complete

---

### 30. Pool and Spa Equipment

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Pools/Pool/Type` | ✅ | ❌ | **MISSING** |
| `Pools/Pool/Pumps/Pump/Type` | ✅ | ❌ | **MISSING** |
| `Pools/Pool/Pumps/Pump/Load` | ✅ | ✅ | Implemented |
| `Pools/Pool/Heater/Type` | ✅ | ❌ | **MISSING** |
| `Pools/Pool/Heater/Load` | ✅ | ✅ | Implemented |
| `Spas/Spa/Type` | ✅ | ❌ | **MISSING** |
| `Spas/Spa/Pumps/Pump/Type` | ✅ | ❌ | **MISSING** |
| `Spas/Spa/Pumps/Pump/Load` | ✅ | ✅ | Implemented |
| `Spas/Spa/Heater/Type` | ✅ | ❌ | **MISSING** |
| `Spas/Spa/Heater/Load` | ✅ | ✅ | Implemented |

**Gap**: Pool/spa types, pump/heater types

---

### 31. Photovoltaics (PV)

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Location` | ✅ | ❌ | **MISSING** |
| `ModuleType` | ✅ | ❌ | **MISSING** |
| `Tracking` | ✅ | ❌ | **MISSING** |
| `ArrayAzimuth` | ✅ | ✅ | Implemented |
| `ArrayTilt` | ✅ | ✅ | Implemented |
| `MaxPowerOutput` | ✅ | ✅ | Implemented |
| `SystemLossesFraction` | ✅ | ❌ | **MISSING** |
| `AttachedToInverter` | ✅ | ❌ | **MISSING** |
| `Inverter/InverterEfficiency` | ✅ | ✅ | Implemented |

**Gap**: PV location, module type, tracking, system losses, inverter linking

---

### 32. Battery Systems

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Location` | ✅ | ❌ | **MISSING** |
| `BatteryType` | ✅ | ❌ | **MISSING** |
| `NominalCapacity/Units` | ✅ | ❌ | **MISSING** |
| `NominalCapacity/Value` | ✅ | Partial | kWh only |
| `UsableCapacity/Units` | ✅ | ❌ | **MISSING** |
| `UsableCapacity/Value` | ✅ | ❌ | **MISSING** |
| `RatedPowerOutput` | ✅ | ✅ | Implemented |
| `RoundTripEfficiency` | ✅ | ✅ | Implemented |

**Gap**: Battery location, type, capacity units handling, usable capacity

---

### 33. Generator Systems

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Generator` | ✅ | ❌ | **MISSING** |

**Gap**: Generator equipment not implemented

---

### 34. Electric Vehicles

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `ElectricVehicle` | ✅ | Partial | Via PlugLoad only |
| `ChargingLevel` | ✅ | Partial | Level 2 only |
| `MaxChargingPower` | ✅ | ❌ | **MISSING** |
| `BatteryCapacity` | ✅ | Derived | From range miles |

**Gap**: Direct EV element, charging level flexibility, max charging power

---

### 35. Dehumidifier

| HPXML Element | OCHRE | HARES | Gap |
|--------------|-------|-------|-----|
| `Dehumidifier` | ✅ | ❌ | **MISSING** |

**Gap**: Dehumidifier equipment not implemented

---

## Key Gaps Summary

### High Priority (Core Functionality)
1. **House Type** (`ResidentialFacilityType`) - Affects defaults and modeling approach
2. **Foundation Conditioning** - Finished vs unfinished basement affects loads
3. **HPXML ID References** - Attic-to-roof, foundation-to-wall links not resolved
4. **Internal Mass** - Partition walls and furniture mass affects thermal dynamics
5. **DSE (Distribution System Efficiency)** - Alternative to duct calculations
6. **PV System Losses** - Important for accurate solar generation
7. **Battery Usable Capacity** - Different from nominal capacity

### Medium Priority (Accuracy Improvements)
1. **Winter Shading Coefficient** - Seasonal window shading variation
2. **Appliance Locations** - Affects zone heat gains
3. **Water Fixture Types** - Shower vs faucet flow rates
4. **Pool/Spa Types** - Affects operation schedules
5. **Lighting Usage Multipliers** - Per-location adjustment
6. **Foundation Wall Depth** - Affects ground coupling

### Low Priority (Nice to Have)
1. **Bathroom Count** - Minor impact on loads
2. **Tank Height** - Minor impact on water heater UA
3. **Carpet Fraction** - Minor impact on slab heat transfer
4. **Generator** - Not common in residential
5. **Dehumidifier** - Niche equipment

---

## Implementation Recommendations

### Phase 1 (Critical)
- [ ] Add `ResidentialFacilityType` to building parser
- [ ] Resolve HPXML ID references (attic, foundation links)
- [ ] Add foundation conditioning status
- [ ] Add internal mass properties (partition walls, furniture)

### Phase 2 (Important)
- [ ] Add DSE support as alternative to duct calculations
- [ ] Add winter shading coefficient for windows
- [ ] Add appliance locations
- [ ] Add battery usable capacity
- [ ] Add PV system losses

### Phase 3 (Enhancement)
- [ ] Add detailed water fixture handling
- [ ] Add pool/spa types
- [ ] Add lighting usage multipliers
- [ ] Add remaining envelope details (carpet, under-slab insulation)

---

## Appendix: Equipment Simulation Usage Analysis

This section analyzes how parsed HPXML properties are actually used in the simulation for both OCHRE and HARES.

### Equipment Instantiation Pipeline

#### OCHRE (Python)
```
HPXML File → hpxml.py parsing → Equipment Properties Dict → Dwelling.__init__() → Equipment Objects
```

Key files:
- `utils/hpxml.py`: Parses HPXML into property dictionaries
- `utils/equipment.py`: Equipment name/type mappings, DSE calculations
- `Equipment/__init__.py`: `EQUIPMENT_BY_NAME` class mapping
- `Dwelling.py`: Creates equipment instances and runs simulation

#### HARES (Rust)
```
HPXML File → hpxml/ parsing → EquipmentSpec → conversions.rs → EquipmentConfig → Registry → Equipment Objects
```

Key files:
- `hares-io/src/hpxml/`: HPXML parsing into EquipmentSpec
- `hares-core/src/dwelling/conversions.rs`: EquipmentSpec → EquipmentConfig
- `hares-equipment/src/registry.rs`: Equipment factory
- `hares-equipment/src/`: Equipment trait implementations

### Equipment Simulation Capabilities Comparison

| Equipment | OCHRE Physics | HARES Physics | Key Differences |
|-----------|---------------|---------------|-----------------|
| **HVAC** | Biquadratic curves, ASHRAE 152 DSE, ideal capacity algorithm | Biquadratic curves, DSE, 5 speed control modes | HARES has more speed modes, better bounds checking |
| **Water Heaters** | 1-2-12 node stratified, dynamic HPWH | 1-12 node stratified, UA-based losses | Similar capabilities |
| **Battery** | Equivalent circuit, rainflow degradation | Quadratic voltage, rainflow + Arrhenius, thermal | HARES has better thermal model |
| **PV** | SAM PVWatts v8, single array | SAM-NOCT, multi-array, soiling, shading, smart inverter | HARES has more advanced features |
| **Generator** | Efficiency curves, load-following | Efficiency curves (3 types), CHP capable | HARES has CHP support |
| **EV** | EVI-Pro PDF-based | Event-driven, V2L/V2G, archetypes | HARES has grid services |
| **Loads** | Scheduled, event-based, daily PDF | Scheduled, event-based (Markov), stochastic | Similar |
| **Ventilation** | Scheduled load | HRV/ERV with effectiveness, bypass | HARES has actual HRV/ERV physics |

### Critical Finding: Which Parsed Properties Are Actually Used?

#### Properties That Matter (Used in Simulation)

**Envelope (All used in both):**
- `Area`, `RValue`, `U-factor`, `SHGC` - Directly used in thermal calculations
- `Azimuth`, `Tilt/Pitch` - Solar gain calculations
- `SolarAbsorptance`, `Emittance` - Radiation heat transfer
- `Zone connections` (interior/exterior adjacent to) - Boundary condition mapping

**HVAC (Most used):**
- `Capacity`, efficiency values (SEER/HSPF/AFUE) - Equipment sizing and performance
- `CompressorType` - Determines speed control mode
- `Setpoint temperatures` - Thermostat control
- `FractionLoadServed` - Equipment responsibility allocation
- `Duct location/leakage/R-value` - DSE calculations (if used)

**Water Heating (Most used):**
- `TankVolume` - Thermal mass
- `EnergyFactor`/`UniformEnergyFactor` - UA calculation
- `SetpointTemperature` - Thermostat control
- `HeatingCapacity` - Recovery rate

**DER (Most used):**
- `MaxPowerOutput` (PV/Battery) - Capacity limits
- `ArrayTilt/Azimuth` (PV) - Solar incident angle
- `InverterEfficiency` - Conversion losses
- `NominalCapacity` (Battery) - Energy capacity
- `RoundTripEfficiency` (Battery) - Charge/discharge losses

#### Properties That Are Parsed But NOT Used in Simulation

**OCHRE parses but may not fully utilize:**
- `LabelElectricRate`, `LabelGasRate`, `LabelAnnualGasCost` (appliances) - Only for rating calculations, not simulation
- `InteriorFinish/Type` (walls, foundation walls, floors) - Stored but thermal model uses R-value only
- `WallType`, `FloorType`, `RoofType` - Used for lookup but thermal model uses effective R-value
- `Window/WinterShadingCoefficient` - OCHRE only uses summer coefficient
- `HoursInOperation` (ventilation) - OCHRE requires 24 hours, doesn't vary by hour
- `HasFlueOrChimneyInConditionedSpace` - Parsed but not connected to infiltration model in current code

**HARES parses but may not fully utilize:**
- Many `extension/*` schedule parameters - Basic schedules implemented, not all extensions
- Detailed appliance rating data (IMEF, CEF, etc.) - Used for load magnitude, not detailed physics
- `WaterFixtureType` - Hot water draw profile uses aggregate load only

#### Properties That Would Matter If Implemented

**High Impact if Added:**
- `ResidentialFacilityType` - Would enable house-type specific defaults (mobile homes have different characteristics)
- `Foundation/Basement/Conditioned` - Would change zone thermal coupling
- `InternalMass` (partition walls, furniture) - Would add thermal capacitance, affecting temperature swing
- `DSE` (Distribution System Efficiency) - Would provide simpler alternative to full duct modeling
- `PVSystemLossesFraction` - Would improve PV generation accuracy
- `BatteryUsableCapacity` - Would improve SOC management vs nominal capacity

**Medium Impact:**
- `WinterShadingCoefficient` - Would improve seasonal window performance
- `ApplianceLocation` - Would improve zone heat gain distribution
- `LightingUsageMultiplier` - Would allow easier calibration
- `CarpetFraction` - Would affect slab thermal response

**Low Impact:**
- `NumberofBathrooms` - Minor hot water usage impact
- `TankHeight` - Minor effect on stratification
- `WaterFixtureType` - Profile differences are small
- `Pool/SpaType` - Schedule variations minimal

### Key Architectural Differences

#### OCHRE Architecture
- **Strengths**: Flexible Python inheritance, extensive defaults database, detailed physics models
- **Weaknesses**: Ideal HVAC algorithm can be unstable at large timesteps, some equipment models are stubs
- **Equipment Creation**: Runtime class resolution from string names
- **Control**: External signals applied via method calls, EBM export for external controllers

#### HARES Architecture
- **Strengths**: Type-safe Rust, efficient trait-based dispatch, checkpointing, multi-array PV, V2G EV
- **Weaknesses**: Some equipment models newer/less validated, fewer chemistry options for batteries
- **Equipment Creation**: Registry pattern with factory functions
- **Control**: Capability-based signal validation, port-based thermal/electrical coupling

### Simulation Execution Order

#### OCHRE (per timestep)
1. Update schedules (weather, occupancy, equipment schedules)
2. Update equipment (calculates power/heat, applies control)
3. Update generators (needs net power from loads)
4. Update envelope (receives all heat gains)
5. Update zone temperatures (fed back to equipment)

#### HARES (per timestep)
1. **Stage 0 (Independent)**: PV, scheduled loads, event loads
2. **Stage 1 (Electrical)**: Battery, EV, generator (reads Stage 0)
3. **Stage 2 (Thermal)**: HVAC, water heater, ventilation (reads zone temps)
4. **Stage 3 (EnvelopeResolution)**: Domain solvers update environment

### Practical Recommendations

Based on this analysis, the **actual simulation impact** of missing HPXML properties:

**Must Implement (Simulation won't work correctly without):**
1. `ResidentialFacilityType` - Mobile homes need different envelope defaults
2. `Foundation/Basement/Conditioned` - Affects zone boundary conditions
3. `DSE` (Distribution System Efficiency) - Alternative to duct calculations many files use

**Should Implement (Noticeable accuracy improvement):**
1. `InternalMass` - Improves thermal capacitance modeling
2. `PVSystemLossesFraction` - 5-20% generation impact
3. `BatteryUsableCapacity` - Improves SOC accuracy vs nominal
4. `WinterShadingCoefficient` - Seasonal window performance

**Nice to Have (Minor impact):**
1. Appliance locations, water fixture types, pool types
2. Carpet fraction, tank height
3. Detailed appliance rating data (label rates/costs)

**Can Defer (Not used in current simulation):**
1. Interior finish types (thermal model uses effective R-value only)
2. Construction type details (wood stud vs CMU - effective R-value dominates)
3. Most detailed schedule extensions (basic 24-hour profiles work)
