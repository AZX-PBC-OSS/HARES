# HPXML Attribute Mapping

This document maps all HPXML attributes that OCHRE reads in HARES, including the XPATH location, the HARES config field they map to, and the source file and line number where parsing happens. Each attribute is classified as either:
- **OCHRE**: Attribute that OCHRE reads from HPXML (for parity with OCHRE)
- **HARES+**: Attribute that HARES reads but OCHRE doesn't (additional capability)

## Table of Contents
1. [Building Site & Geometry](#building-site--geometry)
2. [Envelope - Walls, Roofs, Floors](#envelope---walls-roofs-floors)
3. [Windows](#windows)
4. [Zones](#zones)
5. [Ducts](#ducts)
6. [Infiltration](#infiltration)
7. [HVAC - Heating Systems](#hvac---heating-systems)
8. [HVAC - Cooling Systems](#hvac---cooling-systems)
9. [HVAC - Heat Pumps](#hvac---heat-pumps)
10. [HVAC - Dehumidifier](#hvac---dehumidifier)
11. [HVAC - Thermostat Setpoints](#hvac---thermostat-setpoints)
12. [Water Heaters](#water-heaters)
13. [Photovoltaics (PV)](#photovoltaics-pv)
14. [Battery](#battery)
15. [Electric Vehicle (EV)](#electric-vehicle-ev)
16. [Generator](#generator)
17. [Loads - Occupancy](#loads---occupancy)
18. [Loads - Appliances](#loads---appliances)
19. [Loads - Lighting](#loads---lighting)
20. [Loads - Miscellaneous](#loads---miscellaneous)
21. [Loads - Ventilation](#loads---ventilation)
22. [Loads - Pool/Spa](#loads---poolspa)

---

## Building Site & Geometry

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| Elevation | /BuildingSummary/Site/Elevation | site.elevation_m | building.rs:385 | OCHRE |
| SiteType | /BuildingSummary/Site/SiteType | site.site_type | building.rs:394 | OCHRE |
| ShieldingOfHome | /BuildingSummary/Site/ShieldingOfHome | site.shielding_of_home | building.rs:397 | OCHRE |
| Latitude | /Building/Site/Latitude | site.latitude_deg | building.rs:401 | OCHRE |
| Longitude | /Building/Site/Longitude | site.longitude_deg | building.rs:405 | OCHRE |
| ConditionedFloorArea | /BuildingSummary/BuildingConstruction/ConditionedFloorArea | conditioned floor area | building.rs:410 | OCHRE |
| ConditionedBuildingVolume | /BuildingSummary/BuildingConstruction/ConditionedBuildingVolume | conditioned_volume_m3 | building.rs:414 | OCHRE |
| NumberofConditionedFloors | /BuildingSummary/BuildingConstruction/NumberofConditionedFloors | total_conditioned_floors | building.rs:424 | OCHRE |
| NumberofConditionedFloorsAboveGrade | /BuildingSummary/BuildingConstruction/NumberofConditionedFloorsAboveGrade | floors_above_grade | building.rs:426 | OCHRE |
| ResidentialFacilityType | /BuildingSummary/BuildingConstruction/ResidentialFacilityType | residential_facility_type | building.rs:433 | OCHRE |
| NumberofBedrooms | /BuildingSummary/BuildingConstruction/NumberofBedrooms | n_bedrooms (used in loads) | resolve_loads.rs:50 | OCHRE |
| NumberofResidents | /BuildingSummary/BuildingOccupancy/NumberofResidents | number_of_occupants | resolve_loads.rs:32 | OCHRE |

---

## Envelope - Walls, Roofs, Floors

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| Wall/Area | /Enclosure/Walls/Wall/Area | boundary.area_m2 | building.rs:944 | OCHRE |
| Wall/Azimuth | /Enclosure/Walls/Wall/Azimuth | boundary.azimuth_deg | building.rs:992 | OCHRE |
| Wall/InteriorAdjacentTo | /Enclosure/Walls/Wall/InteriorAdjacentTo | boundary.interior_zone | building.rs:984 | OCHRE |
| Wall/ExteriorAdjacentTo | /Enclosure/Walls/Wall/ExteriorAdjacentTo | boundary.exterior_zone | building.rs:985 | OCHRE |
| Wall/Insulation/Layer/NominalRValue | /Enclosure/Walls/Wall/Insulation/Layer/NominalRValue | boundary.r_value_layers_m2_k_w | building.rs:945 | OCHRE |
| Wall/Insulation/Layer/Thickness | /Enclosure/Walls/Wall/Insulation/Layer/Thickness | material_layer.thickness_m | building.rs:1318 | OCHRE |
| Wall/Insulation/Layer/Conductivity | /Enclosure/Walls/Wall/Insulation/Layer/Conductivity | material_layer.conductivity_w_m_k | building.rs:1319 | OCHRE |
| Wall/Insulation/Layer/Density | /Enclosure/Walls/Wall/Insulation/Layer/Density | material_layer.density_kg_m3 | building.rs:1321 | OCHRE |
| Wall/Insulation/Layer/SpecificHeat | /Enclosure/Walls/Wall/Insulation/Layer/SpecificHeat | material_layer.specific_heat_j_kg_k | building.rs:1322 | OCHRE |
| Wall/WallType | /Enclosure/Walls/Wall/WallType | boundary.construction_type | building.rs:1260 | OCHRE |
| Wall/Siding | /Enclosure/Walls/Wall/Siding | boundary.finish_type | building.rs:1264 | OCHRE |
| Wall/AssemblyEffectiveRValue | /Enclosure/Walls/Wall/AssemblyEffectiveRValue | boundary.assembly_r_value_m2_k_w | building.rs:946 | OCHRE |
| Wall/RValue | /Enclosure/Walls/Wall/RValue | boundary.assembly_r_value_m2_k_w | building.rs:950 | OCHRE |
| Wall/SolarAbsorptance | /Enclosure/Walls/Wall/SolarAbsorptance | boundary.solar_absorptance | building.rs:960 | OCHRE |
| Wall/Emittance | /Enclosure/Walls/Wall/Emittance | boundary.emittance | building.rs:962 | OCHRE |
| Wall/RadiantBarrier | /Enclosure/Walls/Wall/RadiantBarrier | boundary.has_radiant_barrier | building.rs:953 | OCHRE |
| Wall/FramingFactor | /Enclosure/Walls/Wall/FramingFactor | boundary.framing_factor | building.rs:1026 | HARES+ |
| Wall/StudSpacing | /Enclusion/Walls/Wall/StudSpacing | derived framing_factor | building.rs:1034 | HARES+ |
| Wall/StudWidth | /Enclosure/Walls/Wall/StudWidth | derived framing_factor | building.rs:1035 | HARES+ |
| Wall/FloorOrCeiling | /Enclosure/Walls/Wall/FloorOrCeiling | boundary.floor_or_ceiling | building.rs:1007 | OCHRE |
| Roof/Area | /Enclosure/Roofs/Roof/Area | boundary.area_m2 | building.rs:944 | OCHRE |
| Roof/InteriorAdjacentTo | /Enclosure/Roofs/Roof/InteriorAdjacentTo | boundary.interior_zone | building.rs:984 | OCHRE |
| Roof/ExteriorAdjacentTo | /Enclosure/Roofs/Roof/ExteriorAdjacentTo | boundary.exterior_zone | building.rs:985 | OCHRE |
| Roof/Pitch | /Enclosure/Roofs/Roof/Pitch | boundary.tilt_deg | building.rs:974 | OCHRE |
| Roof/RoofType | /Enclosure/Roofs/Roof/RoofType | boundary.finish_type | building.rs:1279 | OCHRE |
| Roof/RadiantBarrier | /Enclosure/Roofs/Roof/RadiantBarrier | boundary.has_radiant_barrier | building.rs:953 | OCHRE |
| Roof/SolarAbsorptance | /Enclosure/Roofs/Roof/SolarAbsorptance | boundary.solar_absorptance | building.rs:960 | OCHRE |
| Roof/Emittance | /Enclosure/Roofs/Roof/Emittance | boundary.emittance | building.rs:962 | OCHRE |
| Floor/Area | /Enclosure/Floors/Floor/Area | boundary.area_m2 | building.rs:944 | OCHRE |
| Floor/InteriorAdjacentTo | /Enclosure/Floors/Floor/InteriorAdjacentTo | boundary.interior_zone | building.rs:984 | OCHRE |
| Floor/ExteriorAdjacentTo | /Enclosure/Floors/Floor/ExteriorAdjacentTo | boundary.exterior_zone | building.rs:985 | OCHRE |
| Floor/FloorType | /Enclosure/Floors/Floor/FloorType | boundary.construction_type | building.rs:1287 | OCHRE |
| FrameFloor/Area | /Enclosure/FrameFloors/FrameFloor/Area | boundary.area_m2 | building.rs:944 | OCHRE |
| FrameFloor/InteriorAdjacentTo | /Enclosure/FrameFloors/FrameFloor/InteriorAdjacentTo | boundary.interior_zone | building.rs:984 | OCHRE |
| FrameFloor/ExteriorAdjacentTo | /Enclosure/FrameFloors/FrameFloor/ExteriorAdjacentTo | boundary.exterior_zone | building.rs:985 | OCHRE |
| Door/Area | /Enclosure/Doors/Door/Area | wall area reduction | building.rs:518 | OCHRE |
| Door/AttachedToWall | /Enclosure/Doors/Door/AttachedToWall | wall area reduction | building.rs:514 | OCHRE |
| RimJoist/Area | /Enclosure/RimJoists/RimJoist/Area | boundary.area_m2 | building.rs:944 | OCHRE |
| RimJoist/InteriorAdjacentTo | /Enclosure/RimJoists/RimJoist/InteriorAdjacentTo | boundary.interior_zone | building.rs:984 | OCHRE |
| RimJoist/ExteriorAdjacentTo | /Enclosure/RimJoists/RimJoist/ExteriorAdjacentTo | boundary.exterior_zone | building.rs:985 | OCHRE |
| FoundationWall/Area | /Enclosure/FoundationWalls/FoundationWall/Area | boundary.area_m2 | building.rs:1059 | OCHRE |
| FoundationWall/Height | /Enclosure/FoundationWalls/FoundationWall/Height | foundation_height_m | building.rs:1138 | OCHRE |
| FoundationWall/DepthBelowGrade | /Enclosure/FoundationWalls/FoundationWall/DepthBelowGrade | area_scale | building.rs:1140 | OCHRE |
| FoundationWall/InteriorAdjacentTo | /Enclosure/FoundationWalls/FoundationWall/InteriorAdjacentTo | boundary.interior_zone | building.rs:984 | OCHRE |
| FoundationWall/ExteriorAdjacentTo | /Enclosure/FoundationWalls/FoundationWall/ExteriorAdjacentTo | boundary.exterior_zone | building.rs:985 | OCHRE |
| FoundationWall/Insulation/Layer/NominalRValue | /Enclosure/FoundationWalls/FoundationWall/Insulation/Layer/NominalRValue | insulation_details | building.rs:1163 | OCHRE |
| FoundationWall/Insulation/Layer/DistanceToTopOfInsulation | /Enclosure/FoundationWalls/FoundationWall/Insulation/Layer/DistanceToTopOfInsulation | insulation_details | building.rs:1169 | OCHRE |
| FoundationWall/Insulation/Layer/DistanceToBottomOfInsulation | /Enclosure/FoundationWalls/FoundationWall/Insulation/Layer/DistanceToBottomOfInsulation | insulation_details | building.rs:1173 | OCHRE |
| FoundationType | /Enclosure/Foundations/Foundation/FoundationType | foundation_name | building.rs:461 | OCHRE |
| Foundation/FloorArea | /Enclosure/Foundations/Foundation/FloorArea | zone floor_area_m2 | building.rs:490 | OCHRE |
| Slab/Area | /Enclosure/Slabs/Slab/Area | boundary.area_m2 | building.rs:1059 | OCHRE |
| Slab/InteriorAdjacentTo | /Enclosure/Slabs/Slab/InteriorAdjacentTo | boundary.interior_zone | building.rs:984 | OCHRE |
| Slab/ExteriorAdjacentTo | /Enclosure/Slabs/Slab/ExteriorAdjacentTo | boundary.exterior_zone | building.rs:985 | OCHRE |
| Slab/PerimeterInsulation/Layer/NominalRValue | /Enclosure/Slabs/Slab/PerimeterInsulation/Layer/NominalRValue | insulation_details | building.rs:1209 | OCHRE |
| Slab/PerimeterInsulation/Layer/InsulationDepth | /Enclosure/Slabs/Slab/PerimeterInsulation/Layer/InsulationDepth | insulation_details | building.rs:1223 | OCHRE |
| Slab/UnderSlabInsulation/Layer/NominalRValue | /Enclosure/Slabs/Slab/UnderSlabInsulation/Layer/NominalRValue | insulation_details | building.rs:1213 | OCHRE |
| Slab/UnderSlabInsulation/Layer/InsulationSpansEntireSlab | /Enclosure/Slabs/Slab/UnderSlabInsulation/Layer/InsulationSpansEntireSlab | insulation_details | building.rs:1232 | OCHRE |
| Slab/UnderSlabInsulation/Layer/InsulationWidth | /Enclosure/Slabs/Slab/UnderSlabInsulation/Layer/InsulationWidth | insulation_details | building.rs:1239 | OCHRE |

---

## Windows

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| Window/Area | /Enclosure/Windows/Window/Area | window.area_m2 | building.rs:856 | OCHRE |
| Window/Azimuth | /Enclosure/Windows/Window/Azimuth | window.azimuth_deg | building.rs:866 | OCHRE |
| Window/UFactor | /Enclosure/Windows/Window/UFactor | window.u_factor_w_m2_k | building.rs:867 | OCHRE |
| Window/SHGC | /Enclosure/Windows/Window/SHGC | window.shgc | building.rs:868 | OCHRE |
| Window/InteriorShading/SummerShadingCoefficient | /Enclosure/Windows/Window/InteriorShading/SummerShadingCoefficient | window.interior_shading_fraction | building.rs:876 | OCHRE |
| Window/InteriorShading/WinterShadingCoefficient | /Enclosure/Windows/Window/InteriorShading/WinterShadingCoefficient | window.winter_shading_fraction | building.rs:881 | OCHRE |
| Window/FractionOperable | /Enclosure/Windows/Window/FractionOperable | window.fraction_operable | building.rs:891 | OCHRE |
| Window/FrameType | /Enclosure/Windows/Window/FrameType | window.frame_type | building.rs:897 | OCHRE |
| Window/AttachedToWall | /Enclosure/Windows/Window/AttachedToWall | window.attached_to_wall_id | building.rs:898 | OCHRE |
| Window/InteriorAdjacentTo | /Enclosure/Windows/Window/InteriorAdjacentTo | boundary.interior_zone | building.rs:922 | OCHRE |
| Window/ExteriorAdjacentTo | /Enclosure/Windows/Window/ExteriorAdjacentTo | boundary.exterior_zone | building.rs:923 | OCHRE |

---

## Zones

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| Attic/FloorArea | /Enclosure/Attics/Attic/FloorArea | zone.floor_area_m2 | building.rs:1420 | OCHRE |
| Attic/AtticType/.../Vented | /Enclosure/Attics/Attic/AtticType/*/Vented | zone.vented | building.rs:1426 | OCHRE |
| Attic/VentilationRate | /Enclosure/Attics/Attic/VentilationRate | zone.ventilation_ach, zone.ventilation_sla | building.rs:1430 | OCHRE |
| Garage/FloorArea | /Enclosure/Garages/Garage/FloorArea | zone.floor_area_m2 | building.rs:1448 | OCHRE |
| Foundation/FloorArea | /Enclosure/Foundations/Foundation/FloorArea | zone.floor_area_m2 | building.rs:1466 | OCHRE |
| Foundation/FoundationType/... | /Enclosure/Foundations/Foundation/FoundationType/* | zone.vented | building.rs:1470 | OCHRE |
| Foundation/VentilationRate | /Enclosure/Foundations/Foundation/VentilationRate | zone.ventilation_ach, zone.ventilation_sla | building.rs:1479 | OCHRE |

---

## Ducts

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| Ducts/SystemIdentifier | /Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/SystemIdentifier | duct.id | building.rs:1591 | OCHRE |
| Ducts/DuctType | /Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/DuctType | duct.duct_type | building.rs:1623 | OCHRE |
| Ducts/DuctInsulationRValue | /Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/DuctInsulationRValue | duct.insulation_r_value_m2_k_w | building.rs:1598 | OCHRE |
| Ducts/InsulationRValue | /Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/InsulationRValue | duct.insulation_r_value_m2_k_w | building.rs:1602 | OCHRE |
| Ducts/DuctSurfaceArea | /Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/DuctSurfaceArea | duct.surface_area_m2 | building.rs:1607 | OCHRE |
| Ducts/DuctLocation | /Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/DuctLocation | duct.location | building.rs:1611 | OCHRE |
| DuctLeakage/DuctType | /Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/DuctLeakageMeasurement/DuctType | leakage_by_type | building.rs:1564 | OCHRE |
| DuctLeakage/Value | /Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/DuctLeakageMeasurement/DuctLeakage/Value | duct.leakage_fraction | building.rs:1573 | OCHRE |
| DuctLeakage/Units | /Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/DuctLeakageMeasurement/DuctLeakage/Units | leakage conversion | building.rs:1577 | OCHRE |

---

## Infiltration

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| InfiltrationHeight | /AirInfiltration/AirInfiltrationMeasurement/InfiltrationHeight | infiltration_height_m | building.rs:439 | OCHRE |
| EffectiveLeakageArea | /AirInfiltration/AirInfiltrationMeasurement/EffectiveLeakageArea | infiltration_ela_cm2 | building.rs:444 | OCHRE |
| AirLeakage (CFM50) | /AirInfiltration/AirInfiltrationMeasurement/AirLeakage (units=CFM50) | infiltration_cfm50 | building.rs:1839 | OCHRE |
| AirLeakage (ACH) | /AirInfiltration/AirInfiltrationMeasurement/AirLeakage (units=ACH) | infiltration_ach50 | building.rs:735 | OCHRE |
| HasFlueOrChimneyInConditionedSpace | /BuildingDetails/extension/HasFlueOrChimneyInConditionedSpace | has_flue_or_chimney | building.rs:455 | OCHRE |

---

## HVAC - Heating Systems

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| HeatingSystem/HeatingSystemFuel | /Systems/HVAC/HeatingSystem/HeatingSystemFuel | fuel_type | resolve_hvac.rs:1192 | OCHRE |
| HeatingSystem/HeatingSystemType | /Systems/HVAC/HeatingSystem/HeatingSystemType | system_type | resolve_hvac.rs:1197 | OCHRE |
| HeatingSystem/HeatingCapacity | /Systems/HVAC/HeatingSystem/HeatingCapacity | heating_capacity_w | resolve_hvac.rs:1203 | OCHRE |
| HeatingSystem/AnnualHeatingEfficiency/Units | /Systems/HVAC/HeatingSystem/AnnualHeatingEfficiency/Units | heating_efficiency_units | resolve_hvac.rs:1630 | OCHRE |
| HeatingSystem/AnnualHeatingEfficiency/Value | /Systems/HVAC/HeatingSystem/AnnualHeatingEfficiency/Value | heating_efficiency | resolve_hvac.rs:1630 | OCHRE |
| HeatingSystem/AFUE | /Systems/HVAC/HeatingSystem/AFUE | efficiency_afue | resolve_hvac.rs:1653 | OCHRE |
| HeatingSystem/COP | /Systems/HVAC/HeatingSystem/COP | efficiency_cop | resolve_hvac.rs:1653 | OCHRE |
| HeatingSystem/FractionHeatLoadServed | /Systems/HVAC/HeatingSystem/FractionHeatLoadServed | fraction_load_served | resolve_hvac.rs:1221 | OCHRE |
| HeatingSystem/FractionHeatingLoadServed | /Systems/HVAC/HeatingSystem/FractionHeatingLoadServed | fraction_load_served | resolve_hvac.rs:1222 | OCHRE |
| HeatingSystem/ElectricAuxiliaryEnergy | /Systems/HVAC/HeatingSystem/ElectricAuxiliaryEnergy | auxiliary_power_w | resolve_hvac.rs:1226 | OCHRE |
| extension/FanPowerWattsPerCFM | /Systems/HVAC/HeatingSystem/extension/FanPowerWattsPerCFM | fan_power_w_per_cfm | resolve_hvac.rs:1234 | OCHRE |
| extension/FanPowerWatts | /Systems/HVAC/HeatingSystem/extension/FanPowerWatts | fan_power_w | resolve_hvac.rs:1236 | OCHRE |
| extension/AirflowDefectRatio | /Systems/HVAC/HeatingSystem/extension/AirflowDefectRatio | airflow_defect_ratio | resolve_hvac.rs:1239 | HARES+ |
| extension/HeatingAirflowCFM | /Systems/HVAC/HeatingSystem/extension/HeatingAirflowCFM | heating_airflow_cfm | resolve_hvac.rs:1242 | HARES+ |

---

## HVAC - Cooling Systems

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| CoolingSystem/CoolingSystemFuel | /Systems/HVAC/CoolingSystem/CoolingSystemFuel | fuel_type | resolve_hvac.rs:1274 | OCHRE |
| CoolingSystem/CoolingSystemType | /Systems/HVAC/CoolingSystem/CoolingSystemType | system_type | resolve_hvac.rs:1279 | OCHRE |
| CoolingSystem/CoolingCapacity | /Systems/HVAC/CoolingSystem/CoolingCapacity | cooling_capacity_w | resolve_hvac.rs:1284 | OCHRE |
| CoolingSystem/AnnualCoolingEfficiency/Units | /Systems/HVAC/CoolingSystem/AnnualCoolingEfficiency/Units | cooling_efficiency_units | resolve_hvac.rs:1630 | OCHRE |
| CoolingSystem/AnnualCoolingEfficiency/Value | /Systems/HVAC/CoolingSystem/AnnualCoolingEfficiency/Value | cooling_efficiency | resolve_hvac.rs:1630 | OCHRE |
| CoolingSystem/SEER | /Systems/HVAC/CoolingSystem/SEER | efficiency_seer | resolve_hvac.rs:1653 | OCHRE |
| CoolingSystem/SEER2 | /Systems/HVAC/CoolingSystem/SEER2 | efficiency_seer | resolve_hvac.rs:1653 | OCHRE |
| CoolingSystem/EER | /Systems/HVAC/CoolingSystem/EER | efficiency_eer | resolve_hvac.rs:1653 | OCHRE |
| CoolingSystem/EER2 | /Systems/HVAC/CoolingSystem/EER2 | efficiency_eer | resolve_hvac.rs:1653 | OCHRE |
| CoolingSystem/CompressorType | /Systems/HVAC/CoolingSystem/CompressorType | speed_control_mode, number_of_speeds | resolve_hvac.rs:1295,1702 | OCHRE |
| CoolingSystem/SensibleHeatFraction | /Systems/HVAC/CoolingSystem/SensibleHeatFraction | shr | resolve_hvac.rs:1301 | OCHRE |
| CoolingSystem/FractionCoolLoadServed | /Systems/HVAC/CoolingSystem/FractionCoolLoadServed | fraction_load_served | resolve_hvac.rs:1304 | OCHRE |
| CoolingSystem/FractionCoolingLoadServed | /Systems/HVAC/CoolingSystem/FractionCoolingLoadServed | fraction_load_served | resolve_hvac.rs:1305 | OCHRE |
| extension/FanPowerWattsPerCFM | /Systems/HVAC/CoolingSystem/extension/FanPowerWattsPerCFM | fan_power_w_per_cfm | resolve_hvac.rs:1310 | OCHRE |
| extension/FanPowerWatts | /Systems/HVAC/CoolingSystem/extension/FanPowerWatts | fan_power_w | resolve_hvac.rs:1312 | OCHRE |
| extension/AirflowDefectRatio | /Systems/HVAC/CoolingSystem/extension/AirflowDefectRatio | airflow_defect_ratio | resolve_hvac.rs:1315 | HARES+ |
| extension/ChargeDefectRatio | /Systems/HVAC/CoolingSystem/extension/ChargeDefectRatio | charge_defect_ratio | resolve_hvac.rs:1318 | HARES+ |
| extension/CoolingAirflowCFM | /Systems/HVAC/CoolingSystem/extension/CoolingAirflowCFM | cooling_airflow_cfm | resolve_hvac.rs:1321 | HARES+ |

---

## HVAC - Heat Pumps

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| HeatPump/HeatPumpType | /Systems/HVAC/HeatPump/HeatPumpType | heat_pump_type | resolve_hvac.rs:1343 | OCHRE |
| HeatPump/HeatingCapacity | /Systems/HVAC/HeatPump/HeatingCapacity | heating_capacity_w | resolve_hvac.rs:1358 | OCHRE |
| HeatPump/CoolingCapacity | /Systems/HVAC/HeatPump/CoolingCapacity | cooling_capacity_w | resolve_hvac.rs:1364 | OCHRE |
| HeatPump/SEER | /Systems/HVAC/HeatPump/SEER | efficiency_seer | resolve_hvac.rs:1653 | OCHRE |
| HeatPump/SEER2 | /Systems/HVAC/HeatPump/SEER2 | efficiency_seer | resolve_hvac.rs:1653 | OCHRE |
| HeatPump/HSPF | /Systems/HVAC/HeatPump/HSPF | efficiency_hspf | resolve_hvac.rs:1653 | OCHRE |
| HeatPump/HSPF2 | /Systems/HVAC/HeatPump/HSPF2 | efficiency_hspf | resolve_hvac.rs:1653 | OCHRE |
| HeatPump/CompressorType | /Systems/HVAC/HeatPump/CompressorType | speed_control_mode, number_of_speeds | resolve_hvac.rs:1422 | OCHRE |
| HeatPump/BackupHeatingCapacity | /Systems/HVAC/HeatPump/BackupHeatingCapacity | backup_capacity_w | resolve_hvac.rs:1378 | OCHRE |
| HeatPump/BackupAnnualHeatingEfficiency/Value | /Systems/HVAC/HeatPump/BackupAnnualHeatingEfficiency/Value | backup_eir | resolve_hvac.rs:1385 | OCHRE |
| HeatPump/BackupSystemFuel | /Systems/HVAC/HeatPump/BackupSystemFuel | backup_fuel | resolve_hvac.rs:1390 | OCHRE |
| HeatPump/CompressorLockoutTemperature | /Systems/HVAC/HeatPump/CompressorLockoutTemperature | hp_lockout_temp_c | resolve_hvac.rs:1396 | OCHRE |
| HeatPump/BackupHeatingSwitchoverTemperature | /Systems/HVAC/HeatPump/BackupHeatingSwitchoverTemperature | hp_lockout_temp_c | resolve_hvac.rs:1396 | OCHRE |
| HeatPump/BackupHeatingLockoutTemperature | /Systems/HVAC/HeatPump/BackupHeatingLockoutTemperature | er_lockout_temp_c | resolve_hvac.rs:1403 | OCHRE |
| HeatPump/CoolingSensibleHeatFraction | /Systems/HVAC/HeatPump/CoolingSensibleHeatFraction | shr | resolve_hvac.rs:1435 | OCHRE |
| HeatPump/FractionHeatingLoadServed | /Systems/HVAC/HeatPump/FractionHeatingLoadServed | fraction_heating_load_served | resolve_hvac.rs:1438 | OCHRE |
| HeatPump/FractionCoolingLoadServed | /Systems/HVAC/HeatPump/FractionCoolingLoadServed | fraction_cooling_load_served | resolve_hvac.rs:1441 | OCHRE |
| extension/FanPowerWattsPerCFM | /Systems/HVAC/HeatPump/extension/FanPowerWattsPerCFM | fan_power_w_per_cfm | resolve_hvac.rs:1445 | OCHRE |
| extension/FanPowerWatts | /Systems/HVAC/HeatPump/extension/FanPowerWatts | fan_power_w | resolve_hvac.rs:1447 | OCHRE |
| extension/AirflowDefectRatio | /Systems/HVAC/HeatPump/extension/AirflowDefectRatio | airflow_defect_ratio | resolve_hvac.rs:1450 | HARES+ |
| extension/ChargeDefectRatio | /Systems/HVAC/HeatPump/extension/ChargeDefectRatio | charge_defect_ratio | resolve_hvac.rs:1453 | HARES+ |
| extension/HeatingAirflowCFM | /Systems/HVAC/HeatPump/extension/HeatingAirflowCFM | heating_airflow_cfm | resolve_hvac.rs:1456 | HARES+ |
| extension/CoolingAirflowCFM | /Systems/HVAC/HeatPump/extension/CoolingAirflowCFM | cooling_airflow_cfm | resolve_hvac.rs:1459 | HARES+ |

---

## HVAC - Dehumidifier

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| Dehumidifier/Capacity | /Systems/HVAC/Dehumidifier/Capacity | capacity_liters_per_day | resolve_hvac.rs:1518 | HARES+ |
| Dehumidifier/EnergyFactor | /Systems/HVAC/Dehumidifier/EnergyFactor | energy_factor | resolve_hvac.rs:1524 | HARES+ |
| Dehumidifier/IntegratedEnergyFactor | /Systems/HVAC/Dehumidifier/IntegratedEnergyFactor | integrated_energy_factor | resolve_hvac.rs:1527 | HARES+ |
| Dehumidifier/FractionDehumidificationLoadServed | /Systems/HVAC/Dehumidifier/FractionDehumidificationLoadServed | fraction_served | resolve_hvac.rs:1530 | HARES+ |
| Dehumidifier/FractionLoadServed | /Systems/HVAC/Dehumidifier/FractionLoadServed | fraction_served | resolve_hvac.rs:1531 | HARES+ |
| Dehumidifier/DehumidistatSetpoint | /Systems/HVAC/Dehumidifier/DehumidistatSetpoint | target_rh | resolve_hvac.rs:1535 | HARES+ |

---

## HVAC - Thermostat Setpoints

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| HVACControl/SetpointTempHeatingSeason | /HVACPlant/HVACControl/SetpointTempHeatingSeason | heating_weekday_setpoints_c, heating_weekend_setpoints_c | building.rs:779, xml_helpers.rs:158 | OCHRE |
| HVACControl/SetpointTempCoolingSeason | /HVACPlant/HVACControl/SetpointTempCoolingSeason | cooling_weekday_setpoints_c, cooling_weekend_setpoints_c | building.rs:782, xml_helpers.rs:158 | OCHRE |
| extension/WeekdaySetpointTempsHeatingSeason | /HVACPlant/HVACControl/extension/WeekdaySetpointTempsHeatingSeason | heating_setpoint_source | xml_helpers.rs:143 | OCHRE |
| extension/WeekendSetpointTempsHeatingSeason | /HVACPlant/HVACControl/extension/WeekendSetpointTempsHeatingSeason | heating_setpoint_source | xml_helpers.rs:143 | OCHRE |
| extension/WeekdaySetpointTempsCoolingSeason | /HVACPlant/HVACControl/extension/WeekdaySetpointTempsCoolingSeason | cooling_setpoint_source | xml_helpers.rs:143 | OCHRE |
| extension/WeekendSetpointTempsCoolingSeason | /HVACPlant/HVACControl/extension/WeekendSetpointTempsCoolingSeason | cooling_setpoint_source | xml_helpers.rs:143 | OCHRE |

---

## Water Heaters

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| WaterHeatingSystem/FuelType | /Systems/WaterHeating/WaterHeatingSystem/FuelType | fuel_type | resolve_water_heater.rs:324 | OCHRE |
| WaterHeatingSystem/WaterHeaterType | /Systems/WaterHeating/WaterHeatingSystem/WaterHeaterType | wh_type | resolve_water_heater.rs:30 | OCHRE |
| WaterHeatingSystem/HotWaterTemperature | /Systems/WaterHeating/WaterHeatingSystem/HotWaterTemperature | setpoint_c | resolve_water_heater.rs:32 | OCHRE |
| WaterHeatingSystem/EnergyFactor | /Systems/WaterHeating/WaterHeatingSystem/EnergyFactor | energy_factor | resolve_water_heater.rs:40 | OCHRE |
| WaterHeatingSystem/UniformEnergyFactor | /Systems/WaterHeating/WaterHeatingSystem/UniformEnergyFactor | uniform_energy_factor | resolve_water_heater.rs:41 | OCHRE |
| WaterHeatingSystem/TankVolume | /Systems/WaterHeating/WaterHeatingSystem/TankVolume | tank_volume_m3 | resolve_water_heater.rs:52 | OCHRE |
| WaterHeatingSystem/TankHeight | /Systems/WaterHeating/WaterHeatingSystem/TankHeight | tank_height_m | resolve_water_heater.rs:54 | OCHRE |
| WaterHeatingSystem/FirstHourRating | /Systems/WaterHeating/WaterHeatingSystem/FirstHourRating | first_hour_rating_m3 | resolve_water_heater.rs:57 | OCHRE |
| WaterHeatingSystem/HeatingCapacity | /Systems/WaterHeating/WaterHeatingSystem/HeatingCapacity | heating_capacity_w | resolve_water_heater.rs:59 | OCHRE |
| WaterHeatingSystem/RecoveryEfficiency | /Systems/WaterHeating/WaterHeatingSystem/RecoveryEfficiency | recovery_efficiency | resolve_water_heater.rs:45 | OCHRE |
| WaterHeatingSystem/PerformanceAdjustment | /Systems/WaterHeating/WaterHeatingSystem/PerformanceAdjustment | performance_adjustment | resolve_water_heater.rs:33 | OCHRE |
| WaterHeatingSystem/Location | /Systems/WaterHeating/WaterHeatingSystem/Location | zone_name | resolve_water_heater.rs:34 | OCHRE |
| WaterHeatingSystem/PilotPower | /Systems/WaterHeating/WaterHeatingSystem/PilotPower | pilot_power_w | resolve_water_heater.rs:106 | OCHRE |
| WaterHeatingSystem/FlueLossFraction | /Systems/WaterHeating/WaterHeatingSystem/FlueLossFraction | flue_loss_fraction | resolve_water_heater.rs:107 | OCHRE |
| WaterFixture/LowFlow | /WaterHeating/WaterFixture/LowFlow | fixture_efficiency | resolve_water_heater.rs:273 | OCHRE |
| HotWaterDistribution/SystemType/Standard/PipingLength | /WaterHeating/HotWaterDistribution/SystemType/Standard/PipingLength | piping_length_m | resolve_water_heater.rs:360 | OCHRE |
| HotWaterDistribution/SystemType/Recirculation/BranchPipingLoopLength | /WaterHeating/HotWaterDistribution/SystemType/Recirculation/BranchPipingLoopLength | branch_loop_length_m | resolve_water_heater.rs:368 | OCHRE |
| HotWaterDistribution/PipeInsulation/PipeRValue | /WaterHeating/HotWaterDistribution/PipeInsulation/PipeRValue | pipe_r_value | resolve_water_heater.rs:348 | OCHRE |
| extension/WaterFixturesUsageMultiplier | /WaterHeating/extension/WaterFixturesUsageMultiplier | usage_multiplier | resolve_water_heater.rs:283 | OCHRE |

---

## Photovoltaics (PV)

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| PVSystem/Tracking | /Systems/Photovoltaics/PVSystem/Tracking | tracking validation | resolve_der.rs:38 | HARES+ |
| PVSystem/MaxPowerOutput | /Systems/Photovoltaics/PVSystem/MaxPowerOutput | capacity_kw | resolve_der.rs:57 | HARES+ |
| PVSystem/ArrayTilt | /Systems/Photovoltaics/PVSystem/ArrayTilt | tilt_deg | resolve_der.rs:60 | HARES+ |
| PVSystem/ArrayAzimuth | /Systems/Photovoltaics/PVSystem/ArrayAzimuth | azimuth_deg | resolve_der.rs:61 | HARES+ |
| PVSystem/ModuleType | /Systems/Photovoltaics/PVSystem/ModuleType | module_type | resolve_der.rs:62 | HARES+ |
| PVSystem/SystemLossesFraction | /Systems/Photovoltaics/PVSystem/SystemLossesFraction | system_losses_fraction | resolve_der.rs:64 | HARES+ |
| PVSystem/AttachedToInverter | /Systems/Photovoltaics/PVSystem/AttachedToInverter | inverter_efficiency | resolve_der.rs:48 | HARES+ |
| Inverter/InverterEfficiency | /Systems/Photovoltaics/Inverter/InverterEfficiency | inverter_efficiency | resolve_der.rs:31 | HARES+ |
| Tilt | /Generation/PVSystem/Tilt | pv_tilt_deg | building.rs:788 | HARES+ |

---

## Battery

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| Battery/NominalCapacity | /Systems/Batteries/Battery/NominalCapacity/Value | capacity_kwh | resolve_der.rs:97 | HARES+ |
| Battery/RatedPowerOutput | /Systems/Batteries/Battery/RatedPowerOutput | max_charge_kw, max_discharge_kw | resolve_der.rs:93 | HARES+ |
| Battery/RoundTripEfficiency | /Systems/Batteries/Battery/RoundTripEfficiency | inverter_efficiency | resolve_der.rs:122 | HARES+ |
| RoundTripEfficiency | /Generation/Battery/RoundTripEfficiency | battery_round_trip_efficiency | building.rs:783 | HARES+ |

---

## Electric Vehicle (EV)

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| ElectricVehicle/ChargingLevel | /Systems/ElectricVehicles/ElectricVehicle/ChargingLevel | charging_level | resolve_der.rs:147 | HARES+ |
| ElectricVehicle/MaxChargingPower | /Systems/ElectricVehicles/ElectricVehicle/MaxChargingPower | max_charging_power_kw | resolve_der.rs:148 | HARES+ |
| ElectricVehicle/BatteryCapacity | /Systems/ElectricVehicles/ElectricVehicle/BatteryCapacity/Value | capacity_kwh | resolve_der.rs:146 | HARES+ |

---

## Generator

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| Generator/FuelType | /extension/Generators/Generator/FuelType | fuel_type | resolve_der.rs:195 | HARES+ |
| Generator/ElectricalPowerOutput | /extension/Generators/Generator/ElectricalPowerOutput | rated_power_kw | resolve_der.rs:215 | HARES+ |
| Generator/AnnualOutputkWh | /extension/Generators/Generator/AnnualOutputkWh | annual_output_kwh | resolve_der.rs:196 | HARES+ |
| Generator/AnnualConsumptionkBtu | /extension/Generators/Generator/AnnualConsumptionkBtu | eta_electric | resolve_der.rs:197 | HARES+ |

---

## Loads - Occupancy

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| BuildingOccupancy/NumberofResidents | /BuildingSummary/BuildingOccupancy/NumberofResidents | number_of_occupants | resolve_loads.rs:32 | OCHRE |
| extension/WeekdayScheduleFractions | /BuildingSummary/BuildingOccupancy/extension/WeekdayScheduleFractions | weekday_schedule_fractions | resolve_loads.rs:790 | OCHRE |
| extension/WeekendScheduleFractions | /BuildingSummary/BuildingOccupancy/extension/WeekendScheduleFractions | weekend_schedule_fractions | resolve_loads.rs:807 | OCHRE |
| extension/UsageMultiplier | /BuildingSummary/BuildingOccupancy/extension/UsageMultiplier | usage_multiplier | resolve_loads.rs:824 | OCHRE |
| extension/MonthlyScheduleMultipliers | /BuildingSummary/BuildingOccupancy/extension/MonthlyScheduleMultipliers | month_multipliers | resolve_loads.rs:834 | OCHRE |

---

## Loads - Appliances

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| ClothesWasher/RatedAnnualkWh | /Appliances/ClothesWasher/RatedAnnualkWh | annual_electric_kwh | resolve_loads.rs:91 | OCHRE |
| ClothesWasher/Capacity | /Appliances/ClothesWasher/Capacity | capacity_m3 | resolve_loads.rs:108 | OCHRE |
| ClothesWasher/LabelUsage | /Appliances/ClothesWasher/LabelUsage | label_usage_cycles_per_week | resolve_loads.rs:114 | OCHRE |
| ClothesWasher/IntegratedModifiedEnergyFactor | /Appliances/ClothesWasher/IntegratedModifiedEnergyFactor | imef | resolve_loads.rs:117 | OCHRE |
| ClothesWasher/FuelType | /Appliances/ClothesWasher/FuelType | fuel_type | resolve_loads.rs:101 | OCHRE |
| ClothesWasher/extension/UsageMultiplier | /Appliances/ClothesWasher/extension/UsageMultiplier | usage_multiplier | resolve_loads.rs:137 | OCHRE |
| ClothesDryer/RatedAnnualkWh | /Appliances/ClothesDryer/RatedAnnualkWh | annual_electric_kwh | resolve_loads.rs:91 | OCHRE |
| ClothesDryer/CombinedEnergyFactor | /Appliances/ClothesDryer/CombinedEnergyFactor | combined_energy_factor | resolve_loads.rs:162 | OCHRE |
| ClothesDryer/EnergyFactor | /Appliances/ClothesDryer/EnergyFactor | energy_factor | resolve_loads.rs:164 | OCHRE |
| ClothesDryer/Vented | /Appliances/ClothesDryer/Vented | vented | resolve_loads.rs:167 | OCHRE |
| ClothesDryer/FuelType | /Appliances/ClothesDryer/FuelType | fuel_type | resolve_loads.rs:101 | OCHRE |
| ClothesDryer/extension/UsageMultiplier | /Appliances/ClothesDryer/extension/UsageMultiplier | usage_multiplier | resolve_loads.rs:201 | OCHRE |
| Dishwasher/RatedAnnualkWh | /Appliances/Dishwasher/RatedAnnualkWh | annual_electric_kwh | resolve_loads.rs:91 | OCHRE |
| Dishwasher/PlaceSettingCapacity | /Appliances/Dishwasher/PlaceSettingCapacity | place_setting_capacity | resolve_loads.rs:231 | OCHRE |
| Dishwasher/LabelUsage | /Appliances/Dishwasher/LabelUsage | label_usage_cycles_per_week | resolve_loads.rs:234 | OCHRE |
| Dishwasher/extension/UsageMultiplier | /Appliances/Dishwasher/extension/UsageMultiplier | usage_multiplier | resolve_loads.rs:253 | OCHRE |
| Refrigerator/RatedAnnualkWh | /Appliances/Refrigerator/RatedAnnualkWh | annual_electric_kwh | resolve_loads.rs:91 | OCHRE |
| Freezer/RatedAnnualkWh | /Appliances/Freezer/RatedAnnualkWh | annual_electric_kwh | resolve_loads.rs:91 | OCHRE |
| CookingRange/IsInduction | /Appliances/CookingRange/IsInduction | burner_ef | resolve_loads.rs:279 | OCHRE |
| CookingRange/FuelType | /Appliances/CookingRange/FuelType | fuel_type | resolve_loads.rs:101 | OCHRE |
| CookingRange/extension/UsageMultiplier | /Appliances/CookingRange/extension/UsageMultiplier | usage_multiplier | resolve_loads.rs:282 | OCHRE |

---

## Loads - Lighting

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| LightingGroup/Location | /Lighting/LightingGroup/Location | location | resolve_loads.rs:319 | OCHRE |
| LightingGroup/LightingType | /Lighting/LightingGroup/LightingType | ty (LED, CFL, fluorescent) | resolve_loads.rs:322 | OCHRE |
| LightingGroup/FractionofUnitsInLocation | /Lighting/LightingGroup/FractionofUnitsInLocation | frac | resolve_loads.rs:323 | OCHRE |
| Lighting/Load | /Lighting/LightingGroup/Load | explicit_kwh | resolve_loads.rs:332 | OCHRE |
| Lighting/extension/*UsageMultiplier | /Lighting/extension/{Location}UsageMultiplier | usage_multiplier | resolve_loads.rs:353 | OCHRE |
| Lighting/extension/MonthlyScheduleMultipliers | /Lighting/extension/MonthlyScheduleMultipliers | month_multipliers | resolve_loads.rs:363 | OCHRE |
| CeilingFan/Count | /Lighting/CeilingFan/Count | count | resolve_loads.rs:382 | OCHRE |
| CeilingFan/Airflow/Efficiency | /Lighting/CeilingFan/Airflow/Efficiency | efficiency_cfm_per_w | resolve_loads.rs:384 | OCHRE |

---

## Loads - Miscellaneous

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| PlugLoad/PlugLoadType | /MiscLoads/PlugLoad/PlugLoadType | load_type | resolve_loads.rs:418 | OCHRE |
| PlugLoad/Load | /MiscLoads/PlugLoad/Load | annual_electric_kwh | resolve_loads.rs:476 | OCHRE |
| FuelLoad/FuelLoadType | /MiscLoads/FuelLoad/FuelLoadType | load_type | resolve_loads.rs:486 | OCHRE |
| FuelLoad/Load | /MiscLoads/FuelLoad/Load | annual_gas_therms | resolve_loads.rs:498 | OCHRE |
| PlugLoad/extension/WeekdayScheduleFractions | /MiscLoads/PlugLoad/extension/WeekdayScheduleFractions | weekday_schedule_fractions | resolve_loads.rs:479 | OCHRE |
| PlugLoad/extension/WeekendScheduleFractions | /MiscLoads/PlugLoad/extension/WeekendScheduleFractions | weekend_schedule_fractions | resolve_loads.rs:479 | OCHRE |
| PlugLoad/extension/UsageMultiplier | /MiscLoads/PlugLoad/extension/UsageMultiplier | usage_multiplier | resolve_loads.rs:479 | OCHRE |
| extension/FracSensible | /.../extension/FracSensible | frac_sensible | resolve_loads.rs:853 | OCHRE |
| extension/FracLatent | /.../extension/FracLatent | frac_latent | resolve_loads.rs:856 | OCHRE |

---

## Loads - Ventilation

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| VentilationFan/UsedForWholeBuildingVentilation | /Systems/MechanicalVentilation/VentilationFans/VentilationFan/UsedForWholeBuildingVentilation | is_whole_building | resolve_loads.rs:579 | OCHRE |
| VentilationFan/UsedForSeasonalCoolingLoadReduction | /Systems/MechanicalVentilation/VentilationFans/VentilationFan/UsedForSeasonalCoolingLoadReduction | is_seasonal_cooling | resolve_loads.rs:581 | OCHRE |
| VentilationFan/FanType | /Systems/MechanicalVentilation/VentilationFans/VentilationFan/FanType | fan_type, ventilation_type, balanced | resolve_loads.rs:588 | OCHRE |
| VentilationFan/RatedFlowRate | /Systems/MechanicalVentilation/VentilationFans/VentilationFan/RatedFlowRate | flow_rate_m3_s | resolve_loads.rs:587 | OCHRE |
| VentilationFan/FanPower | /Systems/MechanicalVentilation/VentilationFans/VentilationFan/FanPower | fan_power_w | resolve_loads.rs:591 | OCHRE |
| VentilationFan/SensibleRecoveryEfficiency | /Systems/MechanicalVentilation/VentilationFans/VentilationFan/SensibleRecoveryEfficiency | sensible_effectiveness | resolve_loads.rs:615 | OCHRE |
| VentilationFan/TotalRecoveryEfficiency | /Systems/MechanicalVentilation/VentilationFans/VentilationFan/TotalRecoveryEfficiency | latent_effectiveness | resolve_loads.rs:616 | OCHRE |
| VentilationFan/HoursInOperation | /Systems/MechanicalVentilation/VentilationFans/VentilationFan/HoursInOperation | hours_in_operation | resolve_loads.rs:631 | OCHRE |

---

## Loads - Pool/Spa

| HPXML Attribute | XPATH | HARES Config Field | File:Line | OCHRE? |
|-----------------|-------|-------------------|-----------|--------|
| Pool/PoolPump/Load | /Pools/Pool/PoolPumps/PoolPump/Load | annual_electric_kwh | resolve_loads.rs:520 | OCHRE |
| Pool/PoolHeater/Load | /Pools/Pool/Heater/Load | annual_electric_kwh / annual_gas_therms | resolve_loads.rs:543 | OCHRE |
| Spa/SpaPump/Load | /Spas/Spa/SpaPumps/SpaPump/Load | annual_electric_kwh | resolve_loads.rs:520 | OCHRE |
| Spa/SpaHeater/Load | /Spas/Spa/SpaHeater/Load | annual_electric_kwh / annual_gas_therms | resolve_loads.rs:543 | OCHRE |

---

## Summary Statistics

- **Total HPXML attributes parsed**: ~200+
- **OCHRE attributes**: ~175 (attributes read by both HARES and OCHRE)
- **HARES+ attributes**: ~30 (additional attributes read by HARES but not by OCHRE)
- **Primary parsing files**: 
  - `building.rs` (envelope, geometry, zones)
  - `resolve_hvac.rs` (HVAC equipment)
  - `resolve_water_heater.rs` (water heaters)
  - `resolve_der.rs` (PV, battery, EV, generator) - all HARES+
  - `resolve_loads.rs` (appliances, lighting, misc loads, ventilation)
  - `xml_helpers.rs` (shared utilities)

---

## Notes

1. **Unit Conversion**: HPXML uses imperial units (ft, F, BTU, etc.) which are converted to SI (m, C, W) during parsing. The conversion functions are in `building.rs` (lines 1717-1833) and `xml_helpers.rs`.

2. **Optional vs Required**: Most HPXML attributes are optional with sensible defaults. Required attributes (like Window Area) will cause parse errors if missing.

3. **Extension Elements**: Many HARES-specific parameters come from HPXML extension elements (e.g., `FanPowerWattsPerCFM`, `AirflowDefectRatio`, schedule fractions).

4. **Typed vs Legacy Config**: Equipment can be configured via either:
   - **Typed config**: Strongly-typed Rust structs (preferred for migrated equipment)
   - **Legacy params**: JSON map with string keys (for older equipment)

5. **OCHRE Parity vs HARES+**: 
   - **OCHRE** attributes have explicit "OCHRE hpxml.py" or "Mirrors OCHRE" comments in the HARES source code
   - **HARES+** attributes are read by HARES but NOT by OCHRE (no OCHRE references in source, verified against OCHRE hpxml.py)
   - Key HARES+ categories: PV, Battery, Generator, EV (via HPXML Systems path), Dehumidifier, HVAC defect ratio extensions, wall framing factors

(End of file - total 611 lines)
