//! The HPXML elements the parser reads, declared once: every
//! element-name literal in the parser is one of these statics, the
//! accessors take them by reference, and the choice dispatches match
//! the nodes against them, so the parser cannot read an element it has
//! not declared. The input schema publishes this list.

/// One HPXML element the parser reads: its name, the parent path the
/// parser reads it under, the unit its value carries when that is
/// fixed (the values read through an HPXML `units` attribute carry
/// `None`), and what the parser reads it for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HpxmlElement {
    pub name: &'static str,
    pub context: &'static str,
    pub unit: Option<hares_types::Unit>,
    pub description: &'static str,
}

/// One HPXML element whose presence fails the parse: HARES does not
/// model it, and accepting it would drop its declaration silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RejectedHpxmlElement {
    pub name: &'static str,
    pub context: &'static str,
    pub reason: &'static str,
}

/// the <AboveApartment> element the parser reads under its parent
pub static ABOVE_APARTMENT: HpxmlElement = HpxmlElement {
    name: "AboveApartment",
    context: "BuildingDetails/Enclosure/Foundations/Foundation/FoundationType/AboveApartment",
    unit: None,
    description: "the <AboveApartment> element the parser reads under its parent",
};

/// the <active_duration_s> element the parser reads under its parent
pub static ACTIVE_DURATION_S: HpxmlElement = HpxmlElement {
    name: "active_duration_s",
    context: "the synthetic extension element's active duration, seconds",
    unit: None,
    description: "the <active_duration_s> element the parser reads under its parent",
};

/// the <active_power_w> element the parser reads under its parent
pub static ACTIVE_POWER_W: HpxmlElement = HpxmlElement {
    name: "active_power_w",
    context: "the synthetic extension element's active power, watts",
    unit: None,
    description: "the <active_power_w> element the parser reads under its parent",
};

/// the <AdjustedAnnualkWh> element the parser reads under its parent
pub static ADJUSTED_ANNUALK_WH: HpxmlElement = HpxmlElement {
    name: "AdjustedAnnualkWh",
    context: "BuildingDetails/Appliances/*/extension (every appliance and load extension)",
    unit: None,
    description: "the <AdjustedAnnualkWh> element the parser reads under its parent",
};

/// the <AdvancedResearchFeatures> element the parser reads under its parent
pub static ADVANCED_RESEARCH_FEATURES: HpxmlElement = HpxmlElement {
    name: "AdvancedResearchFeatures",
    context: "SoftwareInfo/extension/SimulationControl/AdvancedResearchFeatures",
    unit: None,
    description: "the <AdvancedResearchFeatures> element the parser reads under its parent",
};

/// the <Aext> element the parser reads under its parent
pub static AEXT: HpxmlElement = HpxmlElement {
    name: "Aext",
    context: "BuildingDetails/Enclosure/AirInfiltration/AirInfiltrationMeasurement/extension/Aext",
    unit: None,
    description: "the <Aext> element the parser reads under its parent",
};

/// the <AFUE> element the parser reads under its parent
pub static AFUE: HpxmlElement = HpxmlElement {
    name: "AFUE",
    context: "a heating system's AFUE (the legacy direct-child form)",
    unit: None,
    description: "the <AFUE> element the parser reads under its parent",
};

/// the <AirDistribution> element the parser reads under its parent
pub static AIR_DISTRIBUTION: HpxmlElement = HpxmlElement {
    name: "AirDistribution",
    context: "BuildingDetails/Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution",
    unit: None,
    description: "the <AirDistribution> element the parser reads under its parent",
};

/// the <Airflow> element the parser reads under its parent
pub static AIRFLOW: HpxmlElement = HpxmlElement {
    name: "Airflow",
    context: "BuildingDetails/Lighting/CeilingFan/Airflow",
    unit: None,
    description: "the <Airflow> element the parser reads under its parent",
};

/// the <AirflowDefectRatio> element the parser reads under its parent
pub static AIRFLOW_DEFECT_RATIO: HpxmlElement = HpxmlElement {
    name: "AirflowDefectRatio",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/extension/AirflowDefectRatio | BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/extension/AirflowDefectRatio | BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem/extension/AirflowDefectRatio",
    unit: None,
    description: "the <AirflowDefectRatio> element the parser reads under its parent",
};

/// the <AirInfiltrationMeasurement> element the parser reads under its parent
pub static AIR_INFILTRATION_MEASUREMENT: HpxmlElement = HpxmlElement {
    name: "AirInfiltrationMeasurement",
    context: "BuildingDetails/Enclosure/AirInfiltration/AirInfiltrationMeasurement",
    unit: None,
    description: "the <AirInfiltrationMeasurement> element the parser reads under its parent",
};

/// the <AirLeakage> element the parser reads under its parent
pub static AIR_LEAKAGE: HpxmlElement = HpxmlElement {
    name: "AirLeakage",
    context: "BuildingDetails/Enclosure/AirInfiltration/AirInfiltrationMeasurement/BuildingAirLeakage/AirLeakage",
    unit: None,
    description: "the <AirLeakage> element the parser reads under its parent",
};

/// the weather station's altitude
pub static ALTITUDE: HpxmlElement = HpxmlElement {
    name: "Altitude",
    context: "Building/Site (the weather station's altitude; first descendant read)",
    unit: None,
    description: "the weather station's altitude",
};

/// the <Ambient> element the parser reads under its parent
pub static AMBIENT: HpxmlElement = HpxmlElement {
    name: "Ambient",
    context: "BuildingDetails/Enclosure/Foundations/Foundation/FoundationType/Ambient",
    unit: None,
    description: "the <Ambient> element the parser reads under its parent",
};

/// the <Basement> element the parser reads under its parent
pub static BASEMENT: HpxmlElement = HpxmlElement {
    name: "Basement",
    context: "BuildingDetails/Enclosure/Foundations/Foundation/FoundationType/Basement",
    unit: None,
    description: "the <Basement> element the parser reads under its parent",
};

/// the <AnnualConsumptionkBtu> element the parser reads under its parent
pub static ANNUAL_CONSUMPTIONK_BTU: HpxmlElement = HpxmlElement {
    name: "AnnualConsumptionkBtu",
    context: "Systems/extension/Generators/Generator",
    unit: None,
    description: "the <AnnualConsumptionkBtu> element the parser reads under its parent",
};

/// a cooling system's annual efficiency (SEER or EER by units)
pub static ANNUAL_COOLING_EFFICIENCY: HpxmlElement = HpxmlElement {
    name: "AnnualCoolingEfficiency",
    context: "a cooling system's annual efficiency (SEER or EER by units)",
    unit: None,
    description: "a cooling system's annual efficiency (SEER or EER by units)",
};

/// a heating system's annual efficiency (the units attribute names the kind)
pub static ANNUAL_HEATING_EFFICIENCY: HpxmlElement = HpxmlElement {
    name: "AnnualHeatingEfficiency",
    context: "a heating system's annual efficiency (the units attribute names the kind)",
    unit: None,
    description: "a heating system's annual efficiency (the units attribute names the kind)",
};

/// the <AnnualOutputkWh> element the parser reads under its parent
pub static ANNUAL_OUTPUTK_WH: HpxmlElement = HpxmlElement {
    name: "AnnualOutputkWh",
    context: "Systems/extension/Generators/Generator",
    unit: None,
    description: "the <AnnualOutputkWh> element the parser reads under its parent",
};

/// the <Appliances> element the parser reads under its parent
pub static APPLIANCES: HpxmlElement = HpxmlElement {
    name: "Appliances",
    context: "BuildingDetails/Appliances",
    unit: None,
    description: "the <Appliances> element the parser reads under its parent",
};

/// an element's area (an HPXML units attribute carries its unit)
pub static AREA: HpxmlElement = HpxmlElement {
    name: "Area",
    context: "BuildingDetails/Enclosure/Doors/Door/Area | BuildingDetails/Enclosure/Floors/Floor/Area | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/Area | BuildingDetails/Enclosure/RimJoists/RimJoist/Area; and 4 more parent paths in the fixture corpus",
    unit: None,
    description: "an element's area (an HPXML units attribute carries its unit)",
};

/// the <AreaFraction> element the parser reads under its parent
pub static AREA_FRACTION: HpxmlElement = HpxmlElement {
    name: "AreaFraction",
    context: "BuildingDetails/Enclosure/extension/FurnitureMass/AreaFraction | BuildingDetails/Enclosure/extension/PartitionWallMass/AreaFraction",
    unit: None,
    description: "the <AreaFraction> element the parser reads under its parent",
};

/// the PV array's azimuth, degrees
pub static ARRAY_AZIMUTH: HpxmlElement = HpxmlElement {
    name: "ArrayAzimuth",
    context: "BuildingDetails/Systems/Photovoltaics/PVSystem/ArrayAzimuth",
    unit: None,
    description: "the PV array's azimuth, degrees",
};

/// the <ArrayOrientation> element the parser reads under its parent
pub static ARRAY_ORIENTATION: HpxmlElement = HpxmlElement {
    name: "ArrayOrientation",
    context: "BuildingDetails/Systems/Photovoltaics/PVSystem/ArrayOrientation",
    unit: None,
    description: "the <ArrayOrientation> element the parser reads under its parent",
};

/// the PV array's tilt from horizontal, degrees
pub static ARRAY_TILT: HpxmlElement = HpxmlElement {
    name: "ArrayTilt",
    context: "BuildingDetails/Systems/Photovoltaics/PVSystem/ArrayTilt",
    unit: None,
    description: "the PV array's tilt from horizontal, degrees",
};

/// the <AssemblyEffectiveRValue> element the parser reads under its parent
pub static ASSEMBLY_EFFECTIVE_R_VALUE: HpxmlElement = HpxmlElement {
    name: "AssemblyEffectiveRValue",
    context: "BuildingDetails/Enclosure/Floors/Floor/Insulation/AssemblyEffectiveRValue | BuildingDetails/Enclosure/RimJoists/RimJoist/Insulation/AssemblyEffectiveRValue | BuildingDetails/Enclosure/Roofs/Roof/Insulation/AssemblyEffectiveRValue | BuildingDetails/Enclosure/Walls/Wall/Insulation/AssemblyEffectiveRValue",
    unit: None,
    description: "the <AssemblyEffectiveRValue> element the parser reads under its parent",
};

/// the <AttachedToFloor> element the parser reads under its parent
pub static ATTACHED_TO_FLOOR: HpxmlElement = HpxmlElement {
    name: "AttachedToFloor",
    context: "BuildingDetails/Enclosure/Attics/Attic/AttachedToFloor | BuildingDetails/Enclosure/Foundations/Foundation/AttachedToFloor",
    unit: None,
    description: "the <AttachedToFloor> element the parser reads under its parent",
};

/// the <AttachedToHotWaterDistribution> element the parser reads under its parent
pub static ATTACHED_TO_HOT_WATER_DISTRIBUTION: HpxmlElement = HpxmlElement {
    name: "AttachedToHotWaterDistribution",
    context: "WaterHeatingSystem and WaterHeating (the distribution link)",
    unit: None,
    description: "the <AttachedToHotWaterDistribution> element the parser reads under its parent",
};

/// the PV system's inverter link
pub static ATTACHED_TO_INVERTER: HpxmlElement = HpxmlElement {
    name: "AttachedToInverter",
    context: "BuildingDetails/Systems/Photovoltaics/PVSystem/AttachedToInverter",
    unit: None,
    description: "the PV system's inverter link",
};

/// the <AttachedToPool> element the parser reads under its parent
pub static ATTACHED_TO_POOL: HpxmlElement = HpxmlElement {
    name: "AttachedToPool",
    context: "Pool and PermanentSpa (the pool link)",
    unit: None,
    description: "the <AttachedToPool> element the parser reads under its parent",
};

/// the <AttachedToRoof> element the parser reads under its parent
pub static ATTACHED_TO_ROOF: HpxmlElement = HpxmlElement {
    name: "AttachedToRoof",
    context: "BuildingDetails/Enclosure/Attics/Attic/AttachedToRoof",
    unit: None,
    description: "the <AttachedToRoof> element the parser reads under its parent",
};

/// the <AttachedToWall> element the parser reads under its parent
pub static ATTACHED_TO_WALL: HpxmlElement = HpxmlElement {
    name: "AttachedToWall",
    context: "BuildingDetails/Enclosure/Attics/Attic/AttachedToWall | BuildingDetails/Enclosure/Doors/Door/AttachedToWall | BuildingDetails/Enclosure/Foundations/Foundation/AttachedToWall | BuildingDetails/Enclosure/Windows/Window/AttachedToWall",
    unit: None,
    description: "the <AttachedToWall> element the parser reads under its parent",
};

/// the <AttachedToWaterHeatingSystem> element the parser reads under its parent
pub static ATTACHED_TO_WATER_HEATING_SYSTEM: HpxmlElement = HpxmlElement {
    name: "AttachedToWaterHeatingSystem",
    context: "Appliances' load elements (the water-heating link)",
    unit: None,
    description: "the <AttachedToWaterHeatingSystem> element the parser reads under its parent",
};

/// the <Attic> element the parser reads under its parent
pub static ATTIC: HpxmlElement = HpxmlElement {
    name: "Attic",
    context: "BuildingDetails/Enclosure/Attics/Attic | BuildingDetails/Enclosure/Attics/Attic/AtticType/Attic",
    unit: None,
    description: "the <Attic> element the parser reads under its parent",
};

/// the <Attics> element the parser reads under its parent
pub static ATTICS: HpxmlElement = HpxmlElement {
    name: "Attics",
    context: "BuildingDetails/Enclosure/Attics",
    unit: None,
    description: "the <Attics> element the parser reads under its parent",
};

/// the <AtticType> element the parser reads under its parent
pub static ATTIC_TYPE: HpxmlElement = HpxmlElement {
    name: "AtticType",
    context: "BuildingDetails/Enclosure/Attics/Attic/AtticType",
    unit: None,
    description: "the <AtticType> element the parser reads under its parent",
};

/// the <AutosizingLimits> element the parser reads under its parent
pub static AUTOSIZING_LIMITS: HpxmlElement = HpxmlElement {
    name: "AutosizingLimits",
    context: "HVAC system elements and their extension (the autosizing limits)",
    unit: None,
    description: "the <AutosizingLimits> element the parser reads under its parent",
};

/// the <AverageCeilingHeight> element the parser reads under its parent
pub static AVERAGE_CEILING_HEIGHT: HpxmlElement = HpxmlElement {
    name: "AverageCeilingHeight",
    context: "BuildingDetails/BuildingSummary/BuildingConstruction/AverageCeilingHeight",
    unit: None,
    description: "the <AverageCeilingHeight> element the parser reads under its parent",
};

/// an element's facing, degrees clockwise from north
pub static AZIMUTH: HpxmlElement = HpxmlElement {
    name: "Azimuth",
    context: "BuildingDetails/BuildingSummary/Site/extension/Neighbors/NeighborBuilding/Azimuth | BuildingDetails/Enclosure/Doors/Door/Azimuth | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/Azimuth | BuildingDetails/Enclosure/RimJoists/RimJoist/Azimuth; and 3 more parent paths in the fixture corpus",
    unit: None,
    description: "an element's facing, degrees clockwise from north",
};

/// the <BackupAnnualHeatingEfficiency> element the parser reads under its parent
pub static BACKUP_ANNUAL_HEATING_EFFICIENCY: HpxmlElement = HpxmlElement {
    name: "BackupAnnualHeatingEfficiency",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/BackupAnnualHeatingEfficiency",
    unit: None,
    description: "the <BackupAnnualHeatingEfficiency> element the parser reads under its parent",
};

/// the <BackupHeatingAutosizingFactor> element the parser reads under its parent
pub static BACKUP_HEATING_AUTOSIZING_FACTOR: HpxmlElement = HpxmlElement {
    name: "BackupHeatingAutosizingFactor",
    context: "the backup heating autosizing factor (extension)",
    unit: None,
    description: "the <BackupHeatingAutosizingFactor> element the parser reads under its parent",
};

/// the <BackupHeatingCapacity> element the parser reads under its parent
pub static BACKUP_HEATING_CAPACITY: HpxmlElement = HpxmlElement {
    name: "BackupHeatingCapacity",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/BackupHeatingCapacity | BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/BackupHeatingCapacity",
    unit: None,
    description: "the <BackupHeatingCapacity> element the parser reads under its parent",
};

/// the <BackupHeatingLockoutTemperature> element the parser reads under its parent
pub static BACKUP_HEATING_LOCKOUT_TEMPERATURE: HpxmlElement = HpxmlElement {
    name: "BackupHeatingLockoutTemperature",
    context: "the backup heating lockout temperature, degrees Fahrenheit",
    unit: None,
    description: "the <BackupHeatingLockoutTemperature> element the parser reads under its parent",
};

/// the <BackupHeatingSwitchoverTemperature> element the parser reads under its parent
pub static BACKUP_HEATING_SWITCHOVER_TEMPERATURE: HpxmlElement = HpxmlElement {
    name: "BackupHeatingSwitchoverTemperature",
    context: "the backup heating switchover temperature, degrees Fahrenheit",
    unit: None,
    description: "the <BackupHeatingSwitchoverTemperature> element the parser reads under its parent",
};

/// the <BackupSystemFuel> element the parser reads under its parent
pub static BACKUP_SYSTEM_FUEL: HpxmlElement = HpxmlElement {
    name: "BackupSystemFuel",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/BackupSystemFuel",
    unit: None,
    description: "the <BackupSystemFuel> element the parser reads under its parent",
};

/// the <BackupType> element the parser reads under its parent
pub static BACKUP_TYPE: HpxmlElement = HpxmlElement {
    name: "BackupType",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/BackupType",
    unit: None,
    description: "the <BackupType> element the parser reads under its parent",
};

/// the <Crawlspace> element the parser reads under its parent
pub static CRAWLSPACE: HpxmlElement = HpxmlElement {
    name: "Crawlspace",
    context: "BuildingDetails/Enclosure/Foundations/Foundation/FoundationType/Crawlspace",
    unit: None,
    description: "the <Crawlspace> element the parser reads under its parent",
};

/// the <Batteries> element the parser reads under its parent
pub static BATTERIES: HpxmlElement = HpxmlElement {
    name: "Batteries",
    context: "BuildingDetails/Systems/Batteries",
    unit: None,
    description: "the <Batteries> element the parser reads under its parent",
};

/// the <Battery> element the parser reads under its parent
pub static BATTERY: HpxmlElement = HpxmlElement {
    name: "Battery",
    context: "BuildingDetails/Systems/Batteries/Battery | BuildingDetails/Systems/Vehicles/Vehicle/VehicleType/BatteryElectricVehicle/Battery",
    unit: None,
    description: "the <Battery> element the parser reads under its parent",
};

/// the <BatteryCapacity> element the parser reads under its parent
pub static BATTERY_CAPACITY: HpxmlElement = HpxmlElement {
    name: "BatteryCapacity",
    context: "Systems/ElectricVehicles/ElectricVehicle",
    unit: None,
    description: "the <BatteryCapacity> element the parser reads under its parent",
};

/// the <BeginDayOfMonth> element the parser reads under its parent
pub static BEGIN_DAY_OF_MONTH: HpxmlElement = HpxmlElement {
    name: "BeginDayOfMonth",
    context: "an unavailable period's begin day of month",
    unit: None,
    description: "the <BeginDayOfMonth> element the parser reads under its parent",
};

/// the <BeginHourOfDay> element the parser reads under its parent
pub static BEGIN_HOUR_OF_DAY: HpxmlElement = HpxmlElement {
    name: "BeginHourOfDay",
    context: "an unavailable period's begin hour of day",
    unit: None,
    description: "the <BeginHourOfDay> element the parser reads under its parent",
};

/// the <BeginMonth> element the parser reads under its parent
pub static BEGIN_MONTH: HpxmlElement = HpxmlElement {
    name: "BeginMonth",
    context: "an unavailable period's begin month",
    unit: None,
    description: "the <BeginMonth> element the parser reads under its parent",
};

/// the <BranchPipingLoopLength> element the parser reads under its parent
pub static BRANCH_PIPING_LOOP_LENGTH: HpxmlElement = HpxmlElement {
    name: "BranchPipingLoopLength",
    context: "WaterHeatingSystem/HotWaterDistribution/Recirculation",
    unit: None,
    description: "the <BranchPipingLoopLength> element the parser reads under its parent",
};

/// the <Building> element the parser reads under its parent
pub static BUILDING: HpxmlElement = HpxmlElement {
    name: "Building",
    context: "HPXML root (Building/BuildingDetails; the subtree the parser reads)",
    unit: None,
    description: "the <Building> element the parser reads under its parent",
};

/// the <BuildingAirLeakage> element the parser reads under its parent
pub static BUILDING_AIR_LEAKAGE: HpxmlElement = HpxmlElement {
    name: "BuildingAirLeakage",
    context: "BuildingDetails/Enclosure/AirInfiltration/AirInfiltrationMeasurement/BuildingAirLeakage",
    unit: None,
    description: "the <BuildingAirLeakage> element the parser reads under its parent",
};

/// the <BuildingConstruction> element the parser reads under its parent
pub static BUILDING_CONSTRUCTION: HpxmlElement = HpxmlElement {
    name: "BuildingConstruction",
    context: "BuildingDetails/BuildingSummary/BuildingConstruction",
    unit: None,
    description: "the <BuildingConstruction> element the parser reads under its parent",
};

/// the <BuildingDetails> element the parser reads under its parent
pub static BUILDING_DETAILS: HpxmlElement = HpxmlElement {
    name: "BuildingDetails",
    context: "Building/BuildingDetails (the subtree the parser reads)",
    unit: None,
    description: "the <BuildingDetails> element the parser reads under its parent",
};

/// the <BuildingOccupancy> element the parser reads under its parent
pub static BUILDING_OCCUPANCY: HpxmlElement = HpxmlElement {
    name: "BuildingOccupancy",
    context: "BuildingDetails/BuildingSummary/BuildingOccupancy",
    unit: None,
    description: "the <BuildingOccupancy> element the parser reads under its parent",
};

/// the <BuildingSummary> element the parser reads under its parent
pub static BUILDING_SUMMARY: HpxmlElement = HpxmlElement {
    name: "BuildingSummary",
    context: "BuildingDetails/BuildingSummary",
    unit: None,
    description: "the <BuildingSummary> element the parser reads under its parent",
};

/// the <Capacity> element the parser reads under its parent
pub static CAPACITY: HpxmlElement = HpxmlElement {
    name: "Capacity",
    context: "the washer's or appliance's capacity (the units attribute names the kind)",
    unit: None,
    description: "the <Capacity> element the parser reads under its parent",
};

/// the <CeilingFan> element the parser reads under its parent
pub static CEILING_FAN: HpxmlElement = HpxmlElement {
    name: "CeilingFan",
    context: "BuildingDetails/Lighting/CeilingFan",
    unit: None,
    description: "the <CeilingFan> element the parser reads under its parent",
};

/// the <ChargeDefectRatio> element the parser reads under its parent
pub static CHARGE_DEFECT_RATIO: HpxmlElement = HpxmlElement {
    name: "ChargeDefectRatio",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/extension/ChargeDefectRatio | BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/extension/ChargeDefectRatio",
    unit: None,
    description: "the <ChargeDefectRatio> element the parser reads under its parent",
};

/// the <ChargingLevel> element the parser reads under its parent
pub static CHARGING_LEVEL: HpxmlElement = HpxmlElement {
    name: "ChargingLevel",
    context: "BuildingDetails/Systems/ElectricVehicleChargers/ElectricVehicleCharger/ChargingLevel",
    unit: None,
    description: "the <ChargingLevel> element the parser reads under its parent",
};

/// the <ClimateandRiskZones> element the parser reads under its parent
pub static CLIMATEAND_RISK_ZONES: HpxmlElement = HpxmlElement {
    name: "ClimateandRiskZones",
    context: "BuildingDetails/ClimateandRiskZones",
    unit: None,
    description: "the <ClimateandRiskZones> element the parser reads under its parent",
};

/// the IECC climate zone string
pub static CLIMATE_ZONE: HpxmlElement = HpxmlElement {
    name: "ClimateZone",
    context: "BuildingDetails/ClimateandRiskZones/ClimateZoneIECC/ClimateZone",
    unit: None,
    description: "the IECC climate zone string",
};

/// the <ClimateZoneIECC> element the parser reads under its parent
pub static CLIMATE_ZONE_IECC: HpxmlElement = HpxmlElement {
    name: "ClimateZoneIECC",
    context: "BuildingDetails/ClimateandRiskZones/ClimateZoneIECC",
    unit: None,
    description: "the <ClimateZoneIECC> element the parser reads under its parent",
};

/// the <ClothesDryer> element the parser reads under its parent
pub static CLOTHES_DRYER: HpxmlElement = HpxmlElement {
    name: "ClothesDryer",
    context: "BuildingDetails/Appliances/ClothesDryer",
    unit: None,
    description: "the <ClothesDryer> element the parser reads under its parent",
};

/// the <ClothesWasher> element the parser reads under its parent
pub static CLOTHES_WASHER: HpxmlElement = HpxmlElement {
    name: "ClothesWasher",
    context: "BuildingDetails/Appliances/ClothesWasher",
    unit: None,
    description: "the <ClothesWasher> element the parser reads under its parent",
};

/// the <ColumnName> element the parser reads under its parent
pub static COLUMN_NAME: HpxmlElement = HpxmlElement {
    name: "ColumnName",
    context: "an unavailable period's schedule column name (its type)",
    unit: None,
    description: "the <ColumnName> element the parser reads under its parent",
};

/// the <CombinedEnergyFactor> element the parser reads under its parent
pub static COMBINED_ENERGY_FACTOR: HpxmlElement = HpxmlElement {
    name: "CombinedEnergyFactor",
    context: "BuildingDetails/Appliances/ClothesDryer/CombinedEnergyFactor",
    unit: None,
    description: "the <CombinedEnergyFactor> element the parser reads under its parent",
};

/// the <CompressorLockoutTemperature> element the parser reads under its parent
pub static COMPRESSOR_LOCKOUT_TEMPERATURE: HpxmlElement = HpxmlElement {
    name: "CompressorLockoutTemperature",
    context: "the compressor lockout temperature, degrees Fahrenheit",
    unit: None,
    description: "the <CompressorLockoutTemperature> element the parser reads under its parent",
};

/// the <CompressorType> element the parser reads under its parent
pub static COMPRESSOR_TYPE: HpxmlElement = HpxmlElement {
    name: "CompressorType",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/CompressorType | BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/CompressorType",
    unit: None,
    description: "the <CompressorType> element the parser reads under its parent",
};

/// the <Conditioned> element the parser reads under its parent
pub static CONDITIONED: HpxmlElement = HpxmlElement {
    name: "Conditioned",
    context: "BuildingDetails/Enclosure/Foundations/Foundation/FoundationType/Basement/Conditioned",
    unit: None,
    description: "the <Conditioned> element the parser reads under its parent",
};

/// the conditioned building volume
pub static CONDITIONED_BUILDING_VOLUME: HpxmlElement = HpxmlElement {
    name: "ConditionedBuildingVolume",
    context: "BuildingDetails/BuildingSummary/BuildingConstruction/ConditionedBuildingVolume",
    unit: None,
    description: "the conditioned building volume",
};

/// the conditioned floor area
pub static CONDITIONED_FLOOR_AREA: HpxmlElement = HpxmlElement {
    name: "ConditionedFloorArea",
    context: "BuildingDetails/BuildingSummary/BuildingConstruction/ConditionedFloorArea",
    unit: None,
    description: "the conditioned floor area",
};

/// the <Conductivity> element the parser reads under its parent
pub static CONDUCTIVITY: HpxmlElement = HpxmlElement {
    name: "Conductivity",
    context: "BuildingDetails/BuildingSummary/Site/Soil/Conductivity",
    unit: None,
    description: "the <Conductivity> element the parser reads under its parent",
};

/// the <CookingRange> element the parser reads under its parent
pub static COOKING_RANGE: HpxmlElement = HpxmlElement {
    name: "CookingRange",
    context: "BuildingDetails/Appliances/CookingRange",
    unit: None,
    description: "the <CookingRange> element the parser reads under its parent",
};

/// the <cooldown_duration_s> element the parser reads under its parent
pub static COOLDOWN_DURATION_S: HpxmlElement = HpxmlElement {
    name: "cooldown_duration_s",
    context: "the synthetic extension element's cooldown duration, seconds",
    unit: None,
    description: "the <cooldown_duration_s> element the parser reads under its parent",
};

/// the <CoolingAirflowCFM> element the parser reads under its parent
pub static COOLING_AIRFLOW_CFM: HpxmlElement = HpxmlElement {
    name: "CoolingAirflowCFM",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/extension/CoolingAirflowCFM | BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/extension/CoolingAirflowCFM",
    unit: None,
    description: "the <CoolingAirflowCFM> element the parser reads under its parent",
};

/// the <CoolingAutosizingFactor> element the parser reads under its parent
pub static COOLING_AUTOSIZING_FACTOR: HpxmlElement = HpxmlElement {
    name: "CoolingAutosizingFactor",
    context: "the cooling autosizing factor (extension)",
    unit: None,
    description: "the <CoolingAutosizingFactor> element the parser reads under its parent",
};

/// the <CoolingCapacity> element the parser reads under its parent
pub static COOLING_CAPACITY: HpxmlElement = HpxmlElement {
    name: "CoolingCapacity",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/CoolingCapacity | BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/CoolingCapacity",
    unit: None,
    description: "the <CoolingCapacity> element the parser reads under its parent",
};

/// the <CoolingSensibleHeatFraction> element the parser reads under its parent
pub static COOLING_SENSIBLE_HEAT_FRACTION: HpxmlElement = HpxmlElement {
    name: "CoolingSensibleHeatFraction",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/CoolingSensibleHeatFraction",
    unit: None,
    description: "the <CoolingSensibleHeatFraction> element the parser reads under its parent",
};

/// the <CoolingSystem> element the parser reads under its parent
pub static COOLING_SYSTEM: HpxmlElement = HpxmlElement {
    name: "CoolingSystem",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem",
    unit: None,
    description: "the <CoolingSystem> element the parser reads under its parent",
};

/// the <CoolingSystemFuel> element the parser reads under its parent
pub static COOLING_SYSTEM_FUEL: HpxmlElement = HpxmlElement {
    name: "CoolingSystemFuel",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/CoolingSystemFuel",
    unit: None,
    description: "the <CoolingSystemFuel> element the parser reads under its parent",
};

/// the <CoolingSystemType> element the parser reads under its parent
pub static COOLING_SYSTEM_TYPE: HpxmlElement = HpxmlElement {
    name: "CoolingSystemType",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/CoolingSystemType",
    unit: None,
    description: "the <CoolingSystemType> element the parser reads under its parent",
};

/// the <COP> element the parser reads under its parent
pub static COP: HpxmlElement = HpxmlElement {
    name: "COP",
    context: "a heat pump's COP (the legacy direct-child form)",
    unit: None,
    description: "the <COP> element the parser reads under its parent",
};

/// the <Count> element the parser reads under its parent
pub static COUNT: HpxmlElement = HpxmlElement {
    name: "Count",
    context: "BuildingDetails/Lighting/CeilingFan/Count | BuildingDetails/Systems/MechanicalVentilation/VentilationFans/VentilationFan/Count",
    unit: None,
    description: "the <Count> element the parser reads under its parent",
};

/// the <CrankcaseHeaterPowerWatts> element the parser reads under its parent
pub static CRANKCASE_HEATER_POWER_WATTS: HpxmlElement = HpxmlElement {
    name: "CrankcaseHeaterPowerWatts",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/extension/CrankcaseHeaterPowerWatts | BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/extension/CrankcaseHeaterPowerWatts",
    unit: None,
    description: "the <CrankcaseHeaterPowerWatts> element the parser reads under its parent",
};

/// the <CrankcaseHeaterWatts> element the parser reads under its parent
pub static CRANKCASE_HEATER_WATTS: HpxmlElement = HpxmlElement {
    name: "CrankcaseHeaterWatts",
    context: "CoolingSystem (and heat pump) cooling",
    unit: None,
    description: "the <CrankcaseHeaterWatts> element the parser reads under its parent",
};

/// the <Dehumidifier> element the parser reads under its parent
pub static DEHUMIDIFIER: HpxmlElement = HpxmlElement {
    name: "Dehumidifier",
    context: "BuildingDetails/Appliances/Dehumidifier",
    unit: None,
    description: "the <Dehumidifier> element the parser reads under its parent",
};

/// the <DehumidistatSetpoint> element the parser reads under its parent
pub static DEHUMIDISTAT_SETPOINT: HpxmlElement = HpxmlElement {
    name: "DehumidistatSetpoint",
    context: "BuildingDetails/Appliances/Dehumidifier/DehumidistatSetpoint",
    unit: None,
    description: "the <DehumidistatSetpoint> element the parser reads under its parent",
};

/// the <Density> element the parser reads under its parent
pub static DENSITY: HpxmlElement = HpxmlElement {
    name: "Density",
    context: "material Layer (insulation and mass layers)",
    unit: None,
    description: "the <Density> element the parser reads under its parent",
};

/// the <DepthBelowGrade> element the parser reads under its parent
pub static DEPTH_BELOW_GRADE: HpxmlElement = HpxmlElement {
    name: "DepthBelowGrade",
    context: "BuildingDetails/Enclosure/FoundationWalls/FoundationWall/DepthBelowGrade | BuildingDetails/Enclosure/Slabs/Slab/DepthBelowGrade",
    unit: None,
    description: "the <DepthBelowGrade> element the parser reads under its parent",
};

/// the <Dishwasher> element the parser reads under its parent
pub static DISHWASHER: HpxmlElement = HpxmlElement {
    name: "Dishwasher",
    context: "BuildingDetails/Appliances/Dishwasher",
    unit: None,
    description: "the <Dishwasher> element the parser reads under its parent",
};

/// the <DistanceToBottomOfInsulation> element the parser reads under its parent
pub static DISTANCE_TO_BOTTOM_OF_INSULATION: HpxmlElement = HpxmlElement {
    name: "DistanceToBottomOfInsulation",
    context: "BuildingDetails/Enclosure/FoundationWalls/FoundationWall/Insulation/Layer/DistanceToBottomOfInsulation",
    unit: None,
    description: "the <DistanceToBottomOfInsulation> element the parser reads under its parent",
};

/// the <DistanceToTopOfInsulation> element the parser reads under its parent
pub static DISTANCE_TO_TOP_OF_INSULATION: HpxmlElement = HpxmlElement {
    name: "DistanceToTopOfInsulation",
    context: "BuildingDetails/Enclosure/FoundationWalls/FoundationWall/Insulation/Layer/DistanceToTopOfInsulation",
    unit: None,
    description: "the <DistanceToTopOfInsulation> element the parser reads under its parent",
};

/// the <Door> element the parser reads under its parent
pub static DOOR: HpxmlElement = HpxmlElement {
    name: "Door",
    context: "Enclosure/Doors (one door)",
    unit: None,
    description: "the <Door> element the parser reads under its parent",
};

/// the <Doors> element the parser reads under its parent
pub static DOORS: HpxmlElement = HpxmlElement {
    name: "Doors",
    context: "Enclosure (the door container)",
    unit: None,
    description: "the <Doors> element the parser reads under its parent",
};

/// the <DuctInsulationRValue> element the parser reads under its parent
pub static DUCT_INSULATION_R_VALUE: HpxmlElement = HpxmlElement {
    name: "DuctInsulationRValue",
    context: "BuildingDetails/Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/DuctInsulationRValue",
    unit: None,
    description: "the <DuctInsulationRValue> element the parser reads under its parent",
};

/// the <DuctLeakage> element the parser reads under its parent
pub static DUCT_LEAKAGE: HpxmlElement = HpxmlElement {
    name: "DuctLeakage",
    context: "BuildingDetails/Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/DuctLeakageMeasurement/DuctLeakage",
    unit: None,
    description: "the <DuctLeakage> element the parser reads under its parent",
};

/// the <DuctLeakageMeasurement> element the parser reads under its parent
pub static DUCT_LEAKAGE_MEASUREMENT: HpxmlElement = HpxmlElement {
    name: "DuctLeakageMeasurement",
    context: "BuildingDetails/Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/DuctLeakageMeasurement",
    unit: None,
    description: "the <DuctLeakageMeasurement> element the parser reads under its parent",
};

/// the <DuctLocation> element the parser reads under its parent
pub static DUCT_LOCATION: HpxmlElement = HpxmlElement {
    name: "DuctLocation",
    context: "BuildingDetails/Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/DuctLocation",
    unit: None,
    description: "the <DuctLocation> element the parser reads under its parent",
};

/// the <Ducts> element the parser reads under its parent
pub static DUCTS: HpxmlElement = HpxmlElement {
    name: "Ducts",
    context: "BuildingDetails/Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts | BuildingDetails/Systems/HVAC/HVACPlant/extension/DesignLoads/CoolingLatent/Ducts | BuildingDetails/Systems/HVAC/HVACPlant/extension/DesignLoads/CoolingSensible/Ducts | BuildingDetails/Systems/HVAC/HVACPlant/extension/DesignLoads/Heating/Ducts",
    unit: None,
    description: "the <Ducts> element the parser reads under its parent",
};

/// the <DuctSurfaceArea> element the parser reads under its parent
pub static DUCT_SURFACE_AREA: HpxmlElement = HpxmlElement {
    name: "DuctSurfaceArea",
    context: "BuildingDetails/Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/DuctSurfaceArea",
    unit: None,
    description: "the <DuctSurfaceArea> element the parser reads under its parent",
};

/// the <DuctType> element the parser reads under its parent
pub static DUCT_TYPE: HpxmlElement = HpxmlElement {
    name: "DuctType",
    context: "BuildingDetails/Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/DuctLeakageMeasurement/DuctType | BuildingDetails/Systems/HVAC/HVACDistribution/DistributionSystemType/AirDistribution/Ducts/DuctType",
    unit: None,
    description: "the <DuctType> element the parser reads under its parent",
};

/// the <EER> element the parser reads under its parent
pub static EER: HpxmlElement = HpxmlElement {
    name: "EER",
    context: "a cooling system's EER (the legacy direct-child form)",
    unit: None,
    description: "the <EER> element the parser reads under its parent",
};

/// the <EER2> element the parser reads under its parent
pub static EER_2: HpxmlElement = HpxmlElement {
    name: "EER2",
    context: "a cooling system's EER2 (the legacy direct-child form)",
    unit: None,
    description: "the <EER2> element the parser reads under its parent",
};

/// the <EffectiveLeakageArea> element the parser reads under its parent
pub static EFFECTIVE_LEAKAGE_AREA: HpxmlElement = HpxmlElement {
    name: "EffectiveLeakageArea",
    context: "AirInfiltrationMeasurement (first descendant read)",
    unit: None,
    description: "the <EffectiveLeakageArea> element the parser reads under its parent",
};

/// the <Efficiency> element the parser reads under its parent
pub static EFFICIENCY: HpxmlElement = HpxmlElement {
    name: "Efficiency",
    context: "BuildingDetails/Lighting/CeilingFan/Airflow/Efficiency | BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/CoolingDetailedPerformanceData/PerformanceDataPoint/Efficiency",
    unit: None,
    description: "the <Efficiency> element the parser reads under its parent",
};

/// the <ElectricalPowerOutput> element the parser reads under its parent
pub static ELECTRICAL_POWER_OUTPUT: HpxmlElement = HpxmlElement {
    name: "ElectricalPowerOutput",
    context: "Systems/extension/Generators/Generator",
    unit: None,
    description: "the <ElectricalPowerOutput> element the parser reads under its parent",
};

/// the <ElectricAuxiliaryEnergy> element the parser reads under its parent
pub static ELECTRIC_AUXILIARY_ENERGY: HpxmlElement = HpxmlElement {
    name: "ElectricAuxiliaryEnergy",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem/ElectricAuxiliaryEnergy",
    unit: None,
    description: "the <ElectricAuxiliaryEnergy> element the parser reads under its parent",
};

/// the <ElectricVehicle> element the parser reads under its parent
pub static ELECTRIC_VEHICLE: HpxmlElement = HpxmlElement {
    name: "ElectricVehicle",
    context: "Systems/ElectricVehicles",
    unit: None,
    description: "the <ElectricVehicle> element the parser reads under its parent",
};

/// the <ElectricVehicles> element the parser reads under its parent
pub static ELECTRIC_VEHICLES: HpxmlElement = HpxmlElement {
    name: "ElectricVehicles",
    context: "Systems (the EV container)",
    unit: None,
    description: "the <ElectricVehicles> element the parser reads under its parent",
};

/// the site's elevation above sea level
pub static ELEVATION: HpxmlElement = HpxmlElement {
    name: "Elevation",
    context: "Building/Site",
    unit: None,
    description: "the site's elevation above sea level",
};

/// the <Emittance> element the parser reads under its parent
pub static EMITTANCE: HpxmlElement = HpxmlElement {
    name: "Emittance",
    context: "BuildingDetails/Enclosure/RimJoists/RimJoist/Emittance | BuildingDetails/Enclosure/Roofs/Roof/Emittance | BuildingDetails/Enclosure/Walls/Wall/Emittance",
    unit: None,
    description: "the <Emittance> element the parser reads under its parent",
};

/// the <Enclosure> element the parser reads under its parent
pub static ENCLOSURE: HpxmlElement = HpxmlElement {
    name: "Enclosure",
    context: "BuildingDetails/Enclosure",
    unit: None,
    description: "the <Enclosure> element the parser reads under its parent",
};

/// the <EndDayOfMonth> element the parser reads under its parent
pub static END_DAY_OF_MONTH: HpxmlElement = HpxmlElement {
    name: "EndDayOfMonth",
    context: "an unavailable period's end day of month",
    unit: None,
    description: "the <EndDayOfMonth> element the parser reads under its parent",
};

/// the <EndHourOfDay> element the parser reads under its parent
pub static END_HOUR_OF_DAY: HpxmlElement = HpxmlElement {
    name: "EndHourOfDay",
    context: "an unavailable period's end hour of day",
    unit: None,
    description: "the <EndHourOfDay> element the parser reads under its parent",
};

/// the <EndMonth> element the parser reads under its parent
pub static END_MONTH: HpxmlElement = HpxmlElement {
    name: "EndMonth",
    context: "an unavailable period's end month",
    unit: None,
    description: "the <EndMonth> element the parser reads under its parent",
};

/// the <EnergyFactor> element the parser reads under its parent
pub static ENERGY_FACTOR: HpxmlElement = HpxmlElement {
    name: "EnergyFactor",
    context: "BuildingDetails/Appliances/Dehumidifier/EnergyFactor | BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/EnergyFactor",
    unit: None,
    description: "the <EnergyFactor> element the parser reads under its parent",
};

/// the <ERICalculation> element the parser reads under its parent
pub static ERI_CALCULATION: HpxmlElement = HpxmlElement {
    name: "ERICalculation",
    context: "SoftwareInfo/extension (the ERI calculation's version)",
    unit: None,
    description: "the <ERICalculation> element the parser reads under its parent",
};

/// the <event_probability_constant> element the parser reads under its parent
pub static EVENT_PROBABILITY_CONSTANT: HpxmlElement = HpxmlElement {
    name: "event_probability_constant",
    context: "the synthetic extension element's constant event probability",
    unit: None,
    description: "the <event_probability_constant> element the parser reads under its parent; \
                  the value is the probability of starting within one hour of open window, the \
                  per-step chance derived from it and the step length; the same hourly rate \
                  holds for any step of an hour or less (the start count a few percent high \
                  for sub-hourly steps, since starts can coincide within an hour, and low for \
                  supra-hourly steps, since a step longer than the hour starts at most once)",
};

/// the <event_probability_schedule_col> element the parser reads under its parent
pub static EVENT_PROBABILITY_SCHEDULE_COL: HpxmlElement = HpxmlElement {
    name: "event_probability_schedule_col",
    context: "the synthetic extension element's event-probability schedule column",
    unit: None,
    description: "the <event_probability_schedule_col> element the parser reads under its parent",
};

/// the <event_probability_source> element the parser reads under its parent
pub static EVENT_PROBABILITY_SOURCE: HpxmlElement = HpxmlElement {
    name: "event_probability_source",
    context: "the synthetic extension element's event-probability source kind",
    unit: None,
    description: "the <event_probability_source> element the parser reads under its parent",
};

/// the <event_window_schedule_col> element the parser reads under its parent
pub static EVENT_WINDOW_SCHEDULE_COL: HpxmlElement = HpxmlElement {
    name: "event_window_schedule_col",
    context: "the synthetic extension element's event-window schedule column",
    unit: None,
    description: "the <event_window_schedule_col> element the parser reads under its parent",
};

/// the <event_window_source> element the parser reads under its parent
pub static EVENT_WINDOW_SOURCE: HpxmlElement = HpxmlElement {
    name: "event_window_source",
    context: "the synthetic extension element's event-window source kind",
    unit: None,
    description: "the <event_window_source> element the parser reads under its parent",
};

/// the <ExposedPerimeter> element the parser reads under its parent
pub static EXPOSED_PERIMETER: HpxmlElement = HpxmlElement {
    name: "ExposedPerimeter",
    context: "BuildingDetails/Enclosure/Slabs/Slab/ExposedPerimeter",
    unit: None,
    description: "the <ExposedPerimeter> element the parser reads under its parent",
};

/// the HPXML extension element, where non-schema inputs live
pub static EXTENSION: HpxmlElement = HpxmlElement {
    name: "extension",
    context: "BuildingDetails/Appliances/ClothesDryer/extension | BuildingDetails/Appliances/ClothesWasher/extension | BuildingDetails/Appliances/CookingRange/extension | BuildingDetails/Appliances/Dishwasher/extension; and 34 more parent paths in the fixture corpus",
    unit: None,
    description: "the HPXML extension element, where non-schema inputs live",
};

/// the zone the element's exterior faces
pub static EXTERIOR_ADJACENT_TO: HpxmlElement = HpxmlElement {
    name: "ExteriorAdjacentTo",
    context: "BuildingDetails/Enclosure/Floors/Floor/ExteriorAdjacentTo | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/ExteriorAdjacentTo | BuildingDetails/Enclosure/RimJoists/RimJoist/ExteriorAdjacentTo | BuildingDetails/Enclosure/Walls/Wall/ExteriorAdjacentTo",
    unit: None,
    description: "the zone the element's exterior faces",
};

/// the <ExteriorShading> element the parser reads under its parent
pub static EXTERIOR_SHADING: HpxmlElement = HpxmlElement {
    name: "ExteriorShading",
    context: "BuildingDetails/Enclosure/Windows/Window/ExteriorShading",
    unit: None,
    description: "the <ExteriorShading> element the parser reads under its parent",
};

/// the <FanPower> element the parser reads under its parent
pub static FAN_POWER: HpxmlElement = HpxmlElement {
    name: "FanPower",
    context: "BuildingDetails/Systems/MechanicalVentilation/VentilationFans/VentilationFan/FanPower",
    unit: None,
    description: "the <FanPower> element the parser reads under its parent",
};

/// the <FanPowerWatts> element the parser reads under its parent
pub static FAN_POWER_WATTS: HpxmlElement = HpxmlElement {
    name: "FanPowerWatts",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem/extension/FanPowerWatts",
    unit: None,
    description: "the <FanPowerWatts> element the parser reads under its parent",
};

/// the <FanPowerWattsPerCFM> element the parser reads under its parent
pub static FAN_POWER_WATTS_PER_CFM: HpxmlElement = HpxmlElement {
    name: "FanPowerWattsPerCFM",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/extension/FanPowerWattsPerCFM | BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/extension/FanPowerWattsPerCFM | BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem/extension/FanPowerWattsPerCFM",
    unit: None,
    description: "the <FanPowerWattsPerCFM> element the parser reads under its parent",
};

/// the <FanType> element the parser reads under its parent
pub static FAN_TYPE: HpxmlElement = HpxmlElement {
    name: "FanType",
    context: "BuildingDetails/Systems/MechanicalVentilation/VentilationFans/VentilationFan/FanType",
    unit: None,
    description: "the <FanType> element the parser reads under its parent",
};

/// the <FirstHourRating> element the parser reads under its parent
pub static FIRST_HOUR_RATING: HpxmlElement = HpxmlElement {
    name: "FirstHourRating",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/FirstHourRating",
    unit: None,
    description: "the <FirstHourRating> element the parser reads under its parent",
};

/// the <FlatRoof> element the parser reads under its parent
pub static FLAT_ROOF: HpxmlElement = HpxmlElement {
    name: "FlatRoof",
    context: "BuildingDetails/Enclosure/Attics/Attic/AtticType/FlatRoof",
    unit: None,
    description: "the <FlatRoof> element the parser reads under its parent",
};

/// the <Floor> element the parser reads under its parent
pub static FLOOR: HpxmlElement = HpxmlElement {
    name: "Floor",
    context: "Enclosure/Floors (one floor)",
    unit: None,
    description: "the <Floor> element the parser reads under its parent",
};

/// a foundation's or zone's floor area
pub static FLOOR_AREA: HpxmlElement = HpxmlElement {
    name: "FloorArea",
    context: "Foundation/FoundationType floor areas and zone geometry",
    unit: None,
    description: "a foundation's or zone's floor area",
};

/// the <FloorOrCeiling> element the parser reads under its parent
pub static FLOOR_OR_CEILING: HpxmlElement = HpxmlElement {
    name: "FloorOrCeiling",
    context: "BuildingDetails/Enclosure/Floors/Floor/FloorOrCeiling",
    unit: None,
    description: "the <FloorOrCeiling> element the parser reads under its parent",
};

/// the <Floors> element the parser reads under its parent
pub static FLOORS: HpxmlElement = HpxmlElement {
    name: "Floors",
    context: "Enclosure (the floor container)",
    unit: None,
    description: "the <Floors> element the parser reads under its parent",
};

/// the <FloorType> element the parser reads under its parent
pub static FLOOR_TYPE: HpxmlElement = HpxmlElement {
    name: "FloorType",
    context: "BuildingDetails/Enclosure/Floors/Floor/FloorType",
    unit: None,
    description: "the <FloorType> element the parser reads under its parent",
};

/// the <FlueLossFraction> element the parser reads under its parent
pub static FLUE_LOSS_FRACTION: HpxmlElement = HpxmlElement {
    name: "FlueLossFraction",
    context: "WaterHeatingSystem (gas)",
    unit: None,
    description: "the <FlueLossFraction> element the parser reads under its parent",
};

/// the <Foundation> element the parser reads under its parent
pub static FOUNDATION: HpxmlElement = HpxmlElement {
    name: "Foundation",
    context: "BuildingDetails/Enclosure/Foundations/Foundation",
    unit: None,
    description: "the <Foundation> element the parser reads under its parent",
};

/// the <Foundations> element the parser reads under its parent
pub static FOUNDATIONS: HpxmlElement = HpxmlElement {
    name: "Foundations",
    context: "BuildingDetails/Enclosure/Foundations",
    unit: None,
    description: "the <Foundations> element the parser reads under its parent",
};

/// the <FoundationType> element the parser reads under its parent
pub static FOUNDATION_TYPE: HpxmlElement = HpxmlElement {
    name: "FoundationType",
    context: "BuildingDetails/Enclosure/Foundations/Foundation/FoundationType",
    unit: None,
    description: "the <FoundationType> element the parser reads under its parent",
};

/// the <FoundationWall> element the parser reads under its parent
pub static FOUNDATION_WALL: HpxmlElement = HpxmlElement {
    name: "FoundationWall",
    context: "Enclosure/FoundationWalls (one foundation wall)",
    unit: None,
    description: "the <FoundationWall> element the parser reads under its parent",
};

/// the <FoundationWalls> element the parser reads under its parent
pub static FOUNDATION_WALLS: HpxmlElement = HpxmlElement {
    name: "FoundationWalls",
    context: "Enclosure (the foundation-wall container)",
    unit: None,
    description: "the <FoundationWalls> element the parser reads under its parent",
};

/// the <FracLatent> element the parser reads under its parent
pub static FRAC_LATENT: HpxmlElement = HpxmlElement {
    name: "FracLatent",
    context: "BuildingDetails/MiscLoads/FuelLoad/extension/FracLatent | BuildingDetails/MiscLoads/PlugLoad/extension/FracLatent",
    unit: None,
    description: "the <FracLatent> element the parser reads under its parent",
};

/// the <FracRadiant> element the parser reads under its parent
pub static FRAC_RADIANT: HpxmlElement = HpxmlElement {
    name: "FracRadiant",
    context: "appliance and load extension (the radiant gain fraction)",
    unit: None,
    description: "the <FracRadiant> element the parser reads under its parent",
};

/// the <FracSensible> element the parser reads under its parent
pub static FRAC_SENSIBLE: HpxmlElement = HpxmlElement {
    name: "FracSensible",
    context: "BuildingDetails/MiscLoads/FuelLoad/extension/FracSensible | BuildingDetails/MiscLoads/PlugLoad/extension/FracSensible",
    unit: None,
    description: "the <FracSensible> element the parser reads under its parent",
};

/// the fraction of the cooling load the equipment serves
pub static FRACTION_COOLING_LOAD_SERVED: HpxmlElement = HpxmlElement {
    name: "FractionCoolingLoadServed",
    context: "CoolingSystem and heat pump elements",
    unit: None,
    description: "the fraction of the cooling load the equipment serves",
};

/// the <FractionCoolLoadServed> element the parser reads under its parent
pub static FRACTION_COOL_LOAD_SERVED: HpxmlElement = HpxmlElement {
    name: "FractionCoolLoadServed",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/FractionCoolLoadServed | BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/FractionCoolLoadServed",
    unit: None,
    description: "the <FractionCoolLoadServed> element the parser reads under its parent",
};

/// the <FractionDehumidificationLoadServed> element the parser reads under its parent
pub static FRACTION_DEHUMIDIFICATION_LOAD_SERVED: HpxmlElement = HpxmlElement {
    name: "FractionDehumidificationLoadServed",
    context: "BuildingDetails/Appliances/Dehumidifier/FractionDehumidificationLoadServed",
    unit: None,
    description: "the <FractionDehumidificationLoadServed> element the parser reads under its parent",
};

/// the <FractionDHWLoadServed> element the parser reads under its parent
pub static FRACTION_DHW_LOAD_SERVED: HpxmlElement = HpxmlElement {
    name: "FractionDHWLoadServed",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/FractionDHWLoadServed",
    unit: None,
    description: "the <FractionDHWLoadServed> element the parser reads under its parent",
};

/// the fraction of the heating load the equipment serves
pub static FRACTION_HEATING_LOAD_SERVED: HpxmlElement = HpxmlElement {
    name: "FractionHeatingLoadServed",
    context: "HeatingSystem and heat pump elements",
    unit: None,
    description: "the fraction of the heating load the equipment serves",
};

/// the <FractionHeatLoadServed> element the parser reads under its parent
pub static FRACTION_HEAT_LOAD_SERVED: HpxmlElement = HpxmlElement {
    name: "FractionHeatLoadServed",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/FractionHeatLoadServed | BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem/FractionHeatLoadServed",
    unit: None,
    description: "the <FractionHeatLoadServed> element the parser reads under its parent",
};

/// the fraction of the load the equipment serves
pub static FRACTION_LOAD_SERVED: HpxmlElement = HpxmlElement {
    name: "FractionLoadServed",
    context: "Appliance, Dehumidifier and load elements",
    unit: None,
    description: "the fraction of the load the equipment serves",
};

/// the <FractionofUnitsInLocation> element the parser reads under its parent
pub static FRACTIONOF_UNITS_IN_LOCATION: HpxmlElement = HpxmlElement {
    name: "FractionofUnitsInLocation",
    context: "BuildingDetails/Lighting/LightingGroup/FractionofUnitsInLocation",
    unit: None,
    description: "the <FractionofUnitsInLocation> element the parser reads under its parent",
};

/// the <FractionOperable> element the parser reads under its parent
pub static FRACTION_OPERABLE: HpxmlElement = HpxmlElement {
    name: "FractionOperable",
    context: "BuildingDetails/Enclosure/Windows/Window/FractionOperable",
    unit: None,
    description: "the <FractionOperable> element the parser reads under its parent",
};

/// the <FrameFloor> element the parser reads under its parent
pub static FRAME_FLOOR: HpxmlElement = HpxmlElement {
    name: "FrameFloor",
    context: "Enclosure/FrameFloors (one frame floor)",
    unit: None,
    description: "the <FrameFloor> element the parser reads under its parent",
};

/// the <FrameFloors> element the parser reads under its parent
pub static FRAME_FLOORS: HpxmlElement = HpxmlElement {
    name: "FrameFloors",
    context: "Enclosure (the frame-floor container)",
    unit: None,
    description: "the <FrameFloors> element the parser reads under its parent",
};

/// the <FramingFactor> element the parser reads under its parent
pub static FRAMING_FACTOR: HpxmlElement = HpxmlElement {
    name: "FramingFactor",
    context: "BuildingDetails/Enclosure/Floors/Floor/FloorJoists/FramingFactor | BuildingDetails/Enclosure/Roofs/Roof/Rafters/FramingFactor | BuildingDetails/Enclosure/Walls/Wall/Studs/FramingFactor",
    unit: None,
    description: "the <FramingFactor> element the parser reads under its parent",
};

/// the <Freezer> element the parser reads under its parent
pub static FREEZER: HpxmlElement = HpxmlElement {
    name: "Freezer",
    context: "BuildingDetails/Appliances/Freezer",
    unit: None,
    description: "the <Freezer> element the parser reads under its parent",
};

/// the <FuelLoad> element the parser reads under its parent
pub static FUEL_LOAD: HpxmlElement = HpxmlElement {
    name: "FuelLoad",
    context: "BuildingDetails/MiscLoads/FuelLoad",
    unit: None,
    description: "the <FuelLoad> element the parser reads under its parent",
};

/// the <FuelLoadType> element the parser reads under its parent
pub static FUEL_LOAD_TYPE: HpxmlElement = HpxmlElement {
    name: "FuelLoadType",
    context: "BuildingDetails/MiscLoads/FuelLoad/FuelLoadType",
    unit: None,
    description: "the <FuelLoadType> element the parser reads under its parent",
};

/// the <FuelType> element the parser reads under its parent
pub static FUEL_TYPE: HpxmlElement = HpxmlElement {
    name: "FuelType",
    context: "BuildingDetails/Appliances/ClothesDryer/FuelType | BuildingDetails/Appliances/CookingRange/FuelType | BuildingDetails/MiscLoads/FuelLoad/FuelType | BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/FuelType",
    unit: None,
    description: "the <FuelType> element the parser reads under its parent",
};

/// the <FurnitureMass> element the parser reads under its parent
pub static FURNITURE_MASS: HpxmlElement = HpxmlElement {
    name: "FurnitureMass",
    context: "BuildingDetails/Enclosure/extension/FurnitureMass",
    unit: None,
    description: "the <FurnitureMass> element the parser reads under its parent",
};

/// the <Garage> element the parser reads under its parent
pub static GARAGE: HpxmlElement = HpxmlElement {
    name: "Garage",
    context: "Enclosure/Garages (garage zones)",
    unit: None,
    description: "the <Garage> element the parser reads under its parent",
};

/// the <Garages> element the parser reads under its parent
pub static GARAGES: HpxmlElement = HpxmlElement {
    name: "Garages",
    context: "Enclosure",
    unit: None,
    description: "the <Garages> element the parser reads under its parent",
};

/// the <Generator> element the parser reads under its parent
pub static GENERATOR: HpxmlElement = HpxmlElement {
    name: "Generator",
    context: "Systems/extension/Generators",
    unit: None,
    description: "the <Generator> element the parser reads under its parent",
};

/// the <Generators> element the parser reads under its parent
pub static GENERATORS: HpxmlElement = HpxmlElement {
    name: "Generators",
    context: "Systems/extension (the generator container)",
    unit: None,
    description: "the <Generators> element the parser reads under its parent",
};

/// the flue-in-conditioned-space flag (an xs:boolean)
pub static HAS_FLUE_OR_CHIMNEY_IN_CONDITIONED_SPACE: HpxmlElement = HpxmlElement {
    name: "HasFlueOrChimneyInConditionedSpace",
    context: "BuildingDetails/BuildingSummary/BuildingConstruction/extension/HasFlueOrChimneyInConditionedSpace | BuildingDetails/Enclosure/AirInfiltration/extension/HasFlueOrChimneyInConditionedSpace",
    unit: None,
    description: "the flue-in-conditioned-space flag (an xs:boolean)",
};

/// the <Heater> element the parser reads under its parent
pub static HEATER: HpxmlElement = HpxmlElement {
    name: "Heater",
    context: "BuildingDetails/HotTubs/HotTub/Heater | BuildingDetails/Pools/Pool/Heater",
    unit: None,
    description: "the <Heater> element the parser reads under its parent",
};

/// the <HeatingAirflowCFM> element the parser reads under its parent
pub static HEATING_AIRFLOW_CFM: HpxmlElement = HpxmlElement {
    name: "HeatingAirflowCFM",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/extension/HeatingAirflowCFM | BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem/extension/HeatingAirflowCFM",
    unit: None,
    description: "the <HeatingAirflowCFM> element the parser reads under its parent",
};

/// the <HeatingAutosizingFactor> element the parser reads under its parent
pub static HEATING_AUTOSIZING_FACTOR: HpxmlElement = HpxmlElement {
    name: "HeatingAutosizingFactor",
    context: "the heating autosizing factor (extension)",
    unit: None,
    description: "the <HeatingAutosizingFactor> element the parser reads under its parent",
};

/// the <HeatingCapacity> element the parser reads under its parent
pub static HEATING_CAPACITY: HpxmlElement = HpxmlElement {
    name: "HeatingCapacity",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/HeatingCapacity | BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem/HeatingCapacity | BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/HeatingCapacity",
    unit: None,
    description: "the <HeatingCapacity> element the parser reads under its parent",
};

/// the <HeatingCapacity17F> element the parser reads under its parent
pub static HEATING_CAPACITY_17_F: HpxmlElement = HpxmlElement {
    name: "HeatingCapacity17F",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/HeatingCapacity17F",
    unit: None,
    description: "the <HeatingCapacity17F> element the parser reads under its parent",
};

/// the <HeatingSystem> element the parser reads under its parent
pub static HEATING_SYSTEM: HpxmlElement = HpxmlElement {
    name: "HeatingSystem",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem",
    unit: None,
    description: "the <HeatingSystem> element the parser reads under its parent",
};

/// the <HeatingSystemFuel> element the parser reads under its parent
pub static HEATING_SYSTEM_FUEL: HpxmlElement = HpxmlElement {
    name: "HeatingSystemFuel",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem/HeatingSystemFuel",
    unit: None,
    description: "the <HeatingSystemFuel> element the parser reads under its parent",
};

/// the <HeatingSystemType> element the parser reads under its parent
pub static HEATING_SYSTEM_TYPE: HpxmlElement = HpxmlElement {
    name: "HeatingSystemType",
    context: "a heating system's type discriminator element",
    unit: None,
    description: "the <HeatingSystemType> element the parser reads under its parent",
};

/// the <HeatPump> element the parser reads under its parent
pub static HEAT_PUMP: HpxmlElement = HpxmlElement {
    name: "HeatPump",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump",
    unit: None,
    description: "the <HeatPump> element the parser reads under its parent",
};

/// the <HeatPumpType> element the parser reads under its parent
pub static HEAT_PUMP_TYPE: HpxmlElement = HpxmlElement {
    name: "HeatPumpType",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/HeatPumpType",
    unit: None,
    description: "the <HeatPumpType> element the parser reads under its parent",
};

/// the <Height> element the parser reads under its parent
pub static HEIGHT: HpxmlElement = HpxmlElement {
    name: "Height",
    context: "BuildingDetails/Enclosure/FoundationWalls/FoundationWall/Height",
    unit: None,
    description: "the <Height> element the parser reads under its parent",
};

/// the <HotTub> element the parser reads under its parent
pub static HOT_TUB: HpxmlElement = HpxmlElement {
    name: "HotTub",
    context: "BuildingDetails/HotTubs/HotTub",
    unit: None,
    description: "the <HotTub> element the parser reads under its parent",
};

/// the <HotTubPump> element the parser reads under its parent
pub static HOT_TUB_PUMP: HpxmlElement = HpxmlElement {
    name: "HotTubPump",
    context: "BuildingDetails/HotTubs/HotTub/HotTubPumps/HotTubPump",
    unit: None,
    description: "the <HotTubPump> element the parser reads under its parent",
};

/// the <HotTubPumps> element the parser reads under its parent
pub static HOT_TUB_PUMPS: HpxmlElement = HpxmlElement {
    name: "HotTubPumps",
    context: "BuildingDetails/HotTubs/HotTub/HotTubPumps",
    unit: None,
    description: "the <HotTubPumps> element the parser reads under its parent",
};

/// the <HotTubs> element the parser reads under its parent
pub static HOT_TUBS: HpxmlElement = HpxmlElement {
    name: "HotTubs",
    context: "BuildingDetails/HotTubs",
    unit: None,
    description: "the <HotTubs> element the parser reads under its parent",
};

/// the <HotWaterDistribution> element the parser reads under its parent
pub static HOT_WATER_DISTRIBUTION: HpxmlElement = HpxmlElement {
    name: "HotWaterDistribution",
    context: "BuildingDetails/Systems/WaterHeating/HotWaterDistribution",
    unit: None,
    description: "the <HotWaterDistribution> element the parser reads under its parent",
};

/// the <HotWaterTemperature> element the parser reads under its parent
pub static HOT_WATER_TEMPERATURE: HpxmlElement = HpxmlElement {
    name: "HotWaterTemperature",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/HotWaterTemperature",
    unit: None,
    description: "the <HotWaterTemperature> element the parser reads under its parent",
};

/// the <HoursInOperation> element the parser reads under its parent
pub static HOURS_IN_OPERATION: HpxmlElement = HpxmlElement {
    name: "HoursInOperation",
    context: "BuildingDetails/Systems/MechanicalVentilation/VentilationFans/VentilationFan/HoursInOperation",
    unit: None,
    description: "the <HoursInOperation> element the parser reads under its parent",
};

/// the document root the parse requires
pub static HPXML: HpxmlElement = HpxmlElement {
    name: "HPXML",
    context: "the document root (the parse requires it)",
    unit: None,
    description: "the document root the parse requires",
};

/// the <HSPF> element the parser reads under its parent
pub static HSPF: HpxmlElement = HpxmlElement {
    name: "HSPF",
    context: "a heat pump's HSPF (the legacy direct-child form)",
    unit: None,
    description: "the <HSPF> element the parser reads under its parent",
};

/// the <HSPF2> element the parser reads under its parent
pub static HSPF_2: HpxmlElement = HpxmlElement {
    name: "HSPF2",
    context: "a heat pump's HSPF2 (the legacy direct-child form)",
    unit: None,
    description: "the <HSPF2> element the parser reads under its parent",
};

/// the <HVAC> element the parser reads under its parent
pub static HVAC: HpxmlElement = HpxmlElement {
    name: "HVAC",
    context: "BuildingDetails/Systems/HVAC",
    unit: None,
    description: "the <HVAC> element the parser reads under its parent",
};

/// the <HVACControl> element the parser reads under its parent
pub static HVAC_CONTROL: HpxmlElement = HpxmlElement {
    name: "HVACControl",
    context: "BuildingDetails/Systems/HVAC/HVACControl",
    unit: None,
    description: "the <HVACControl> element the parser reads under its parent",
};

/// the <HVACPlant> element the parser reads under its parent
pub static HVAC_PLANT: HpxmlElement = HpxmlElement {
    name: "HVACPlant",
    context: "BuildingDetails/Systems/HVAC/HVACPlant",
    unit: None,
    description: "the <HVACPlant> element the parser reads under its parent",
};

/// the <IgnitionType> element the parser reads under its parent
pub static IGNITION_TYPE: HpxmlElement = HpxmlElement {
    name: "IgnitionType",
    context: "WaterHeatingSystem (gas)",
    unit: None,
    description: "the <IgnitionType> element the parser reads under its parent",
};

/// the <InfiltrationHeight> element the parser reads under its parent
pub static INFILTRATION_HEIGHT: HpxmlElement = HpxmlElement {
    name: "InfiltrationHeight",
    context: "BuildingDetails/Enclosure/AirInfiltration/AirInfiltrationMeasurement/InfiltrationHeight",
    unit: None,
    description: "the <InfiltrationHeight> element the parser reads under its parent",
};

/// the <Insulation> element the parser reads under its parent
pub static INSULATION: HpxmlElement = HpxmlElement {
    name: "Insulation",
    context: "BuildingDetails/Enclosure/Floors/Floor/Insulation | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/Insulation | BuildingDetails/Enclosure/RimJoists/RimJoist/Insulation | BuildingDetails/Enclosure/Roofs/Roof/Insulation; and 1 more parent paths in the fixture corpus",
    unit: None,
    description: "the <Insulation> element the parser reads under its parent",
};

/// the <InsulationDepth> element the parser reads under its parent
pub static INSULATION_DEPTH: HpxmlElement = HpxmlElement {
    name: "InsulationDepth",
    context: "BuildingDetails/Enclosure/Slabs/Slab/PerimeterInsulation/Layer/InsulationDepth",
    unit: None,
    description: "the <InsulationDepth> element the parser reads under its parent",
};

/// the <InsulationRValue> element the parser reads under its parent
pub static INSULATION_R_VALUE: HpxmlElement = HpxmlElement {
    name: "InsulationRValue",
    context: "duct and insulation elements (first descendant read)",
    unit: None,
    description: "the <InsulationRValue> element the parser reads under its parent",
};

/// the <InsulationSpansEntireSlab> element the parser reads under its parent
pub static INSULATION_SPANS_ENTIRE_SLAB: HpxmlElement = HpxmlElement {
    name: "InsulationSpansEntireSlab",
    context: "BuildingDetails/Enclosure/Slabs/Slab/UnderSlabInsulation/Layer/InsulationSpansEntireSlab",
    unit: None,
    description: "the <InsulationSpansEntireSlab> element the parser reads under its parent",
};

/// the <InsulationWidth> element the parser reads under its parent
pub static INSULATION_WIDTH: HpxmlElement = HpxmlElement {
    name: "InsulationWidth",
    context: "BuildingDetails/Enclosure/Slabs/Slab/ExteriorHorizontalInsulation/Layer/InsulationWidth | BuildingDetails/Enclosure/Slabs/Slab/UnderSlabInsulation/Layer/InsulationWidth",
    unit: None,
    description: "the <InsulationWidth> element the parser reads under its parent",
};

/// the dehumidifier's or appliance's integrated energy factor
pub static INTEGRATED_ENERGY_FACTOR: HpxmlElement = HpxmlElement {
    name: "IntegratedEnergyFactor",
    context: "Dehumidifier and appliance elements",
    unit: None,
    description: "the dehumidifier's or appliance's integrated energy factor",
};

/// the <IntegratedModifiedEnergyFactor> element the parser reads under its parent
pub static INTEGRATED_MODIFIED_ENERGY_FACTOR: HpxmlElement = HpxmlElement {
    name: "IntegratedModifiedEnergyFactor",
    context: "BuildingDetails/Appliances/ClothesWasher/IntegratedModifiedEnergyFactor",
    unit: None,
    description: "the <IntegratedModifiedEnergyFactor> element the parser reads under its parent",
};

/// the zone the element's interior faces
pub static INTERIOR_ADJACENT_TO: HpxmlElement = HpxmlElement {
    name: "InteriorAdjacentTo",
    context: "BuildingDetails/Enclosure/Floors/Floor/InteriorAdjacentTo | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/InteriorAdjacentTo | BuildingDetails/Enclosure/RimJoists/RimJoist/InteriorAdjacentTo | BuildingDetails/Enclosure/Roofs/Roof/InteriorAdjacentTo; and 2 more parent paths in the fixture corpus",
    unit: None,
    description: "the zone the element's interior faces",
};

/// the <InteriorShading> element the parser reads under its parent
pub static INTERIOR_SHADING: HpxmlElement = HpxmlElement {
    name: "InteriorShading",
    context: "BuildingDetails/Enclosure/Windows/Window/InteriorShading",
    unit: None,
    description: "the <InteriorShading> element the parser reads under its parent",
};

/// the <Inverter> element the parser reads under its parent
pub static INVERTER: HpxmlElement = HpxmlElement {
    name: "Inverter",
    context: "BuildingDetails/Systems/Photovoltaics/Inverter",
    unit: None,
    description: "the <Inverter> element the parser reads under its parent",
};

/// the <InverterEfficiency> element the parser reads under its parent
pub static INVERTER_EFFICIENCY: HpxmlElement = HpxmlElement {
    name: "InverterEfficiency",
    context: "BuildingDetails/Systems/Photovoltaics/Inverter/InverterEfficiency",
    unit: None,
    description: "the <InverterEfficiency> element the parser reads under its parent",
};

/// the <IsInduction> element the parser reads under its parent
pub static IS_INDUCTION: HpxmlElement = HpxmlElement {
    name: "IsInduction",
    context: "BuildingDetails/Appliances/CookingRange/IsInduction",
    unit: None,
    description: "the <IsInduction> element the parser reads under its parent",
};

/// the <IsSharedAppliance> element the parser reads under its parent
pub static IS_SHARED_APPLIANCE: HpxmlElement = HpxmlElement {
    name: "IsSharedAppliance",
    context: "BuildingDetails/Appliances/ClothesDryer/IsSharedAppliance | BuildingDetails/Appliances/ClothesWasher/IsSharedAppliance | BuildingDetails/Appliances/Dishwasher/IsSharedAppliance",
    unit: None,
    description: "the <IsSharedAppliance> element the parser reads under its parent",
};

/// the <Jacket> element the parser reads under its parent
pub static JACKET: HpxmlElement = HpxmlElement {
    name: "Jacket",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/WaterHeaterInsulation/Jacket",
    unit: None,
    description: "the <Jacket> element the parser reads under its parent",
};

/// the <JacketRValue> element the parser reads under its parent
pub static JACKET_R_VALUE: HpxmlElement = HpxmlElement {
    name: "JacketRValue",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/WaterHeaterInsulation/Jacket/JacketRValue",
    unit: None,
    description: "the <JacketRValue> element the parser reads under its parent",
};

/// the <LabelAnnualGasCost> element the parser reads under its parent
pub static LABEL_ANNUAL_GAS_COST: HpxmlElement = HpxmlElement {
    name: "LabelAnnualGasCost",
    context: "the label's annual gas cost ($)",
    unit: None,
    description: "the <LabelAnnualGasCost> element the parser reads under its parent",
};

/// the <LabelElectricRate> element the parser reads under its parent
pub static LABEL_ELECTRIC_RATE: HpxmlElement = HpxmlElement {
    name: "LabelElectricRate",
    context: "the label's electric rate ($/kWh)",
    unit: None,
    description: "the <LabelElectricRate> element the parser reads under its parent",
};

/// the <LabelGasRate> element the parser reads under its parent
pub static LABEL_GAS_RATE: HpxmlElement = HpxmlElement {
    name: "LabelGasRate",
    context: "the label's gas rate ($/therm)",
    unit: None,
    description: "the <LabelGasRate> element the parser reads under its parent",
};

/// the <LabelUsage> element the parser reads under its parent
pub static LABEL_USAGE: HpxmlElement = HpxmlElement {
    name: "LabelUsage",
    context: "the clothes washer's or dishwasher's label usage (kWh/year)",
    unit: None,
    description: "the <LabelUsage> element the parser reads under its parent",
};

/// the <latent_gain_fraction> element the parser reads under its parent
pub static LATENT_GAIN_FRACTION: HpxmlElement = HpxmlElement {
    name: "latent_gain_fraction",
    context: "the latent gain fraction (the synthetic extension element)",
    unit: None,
    description: "the <latent_gain_fraction> element the parser reads under its parent",
};

/// the site's latitude, degrees
pub static LATITUDE: HpxmlElement = HpxmlElement {
    name: "Latitude",
    context: "Building/Site",
    unit: None,
    description: "the site's latitude, degrees",
};

/// one layer of an assembly's insulation or mass
pub static LAYER: HpxmlElement = HpxmlElement {
    name: "Layer",
    context: "BuildingDetails/Enclosure/Floors/Floor/Insulation/Layer | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/Insulation/Layer | BuildingDetails/Enclosure/Roofs/Roof/Insulation/Layer | BuildingDetails/Enclosure/Slabs/Slab/ExteriorHorizontalInsulation/Layer; and 3 more parent paths in the fixture corpus",
    unit: None,
    description: "one layer of an assembly's insulation or mass",
};

/// the <Length> element the parser reads under its parent
pub static LENGTH: HpxmlElement = HpxmlElement {
    name: "Length",
    context: "duct and geometry elements (Length)",
    unit: None,
    description: "the <Length> element the parser reads under its parent",
};

/// the <Lighting> element the parser reads under its parent
pub static LIGHTING: HpxmlElement = HpxmlElement {
    name: "Lighting",
    context: "BuildingDetails/Lighting",
    unit: None,
    description: "the <Lighting> element the parser reads under its parent",
};

/// the <LightingGroup> element the parser reads under its parent
pub static LIGHTING_GROUP: HpxmlElement = HpxmlElement {
    name: "LightingGroup",
    context: "BuildingDetails/Lighting/LightingGroup",
    unit: None,
    description: "the <LightingGroup> element the parser reads under its parent",
};

/// the <LightingType> element the parser reads under its parent
pub static LIGHTING_TYPE: HpxmlElement = HpxmlElement {
    name: "LightingType",
    context: "BuildingDetails/Lighting/LightingGroup/LightingType",
    unit: None,
    description: "the <LightingType> element the parser reads under its parent",
};

/// an appliance's or pool's annual energy load
pub static LOAD: HpxmlElement = HpxmlElement {
    name: "Load",
    context: "BuildingDetails/HotTubs/HotTub/Heater/Load | BuildingDetails/HotTubs/HotTub/HotTubPumps/HotTubPump/Load | BuildingDetails/Lighting/LightingGroup/Load | BuildingDetails/MiscLoads/FuelLoad/Load; and 4 more parent paths in the fixture corpus",
    unit: None,
    description: "an appliance's or pool's annual energy load",
};

/// the zone or space the equipment or load sits in
pub static LOCATION: HpxmlElement = HpxmlElement {
    name: "Location",
    context: "BuildingDetails/Appliances/ClothesDryer/Location | BuildingDetails/Appliances/ClothesWasher/Location | BuildingDetails/Appliances/CookingRange/Location | BuildingDetails/Appliances/Dehumidifier/Location; and 8 more parent paths in the fixture corpus",
    unit: None,
    description: "the zone or space the equipment or load sits in",
};

/// the site's longitude, degrees
pub static LONGITUDE: HpxmlElement = HpxmlElement {
    name: "Longitude",
    context: "Building/Site",
    unit: None,
    description: "the site's longitude, degrees",
};

/// the <LowFlow> element the parser reads under its parent
pub static LOW_FLOW: HpxmlElement = HpxmlElement {
    name: "LowFlow",
    context: "BuildingDetails/Systems/WaterHeating/WaterFixture/LowFlow",
    unit: None,
    description: "the <LowFlow> element the parser reads under its parent",
};

/// the <MaxCapacity> element the parser reads under its parent
pub static MAX_CAPACITY: HpxmlElement = HpxmlElement {
    name: "MaxCapacity",
    context: "an AutosizingLimits element's maximum capacity (Btu/h)",
    unit: None,
    description: "the <MaxCapacity> element the parser reads under its parent",
};

/// the <MaxChargingPower> element the parser reads under its parent
pub static MAX_CHARGING_POWER: HpxmlElement = HpxmlElement {
    name: "MaxChargingPower",
    context: "Systems/ElectricVehicles/ElectricVehicle",
    unit: None,
    description: "the <MaxChargingPower> element the parser reads under its parent",
};

/// the PV system's or inverter's maximum power output, watts
pub static MAX_POWER_OUTPUT: HpxmlElement = HpxmlElement {
    name: "MaxPowerOutput",
    context: "BuildingDetails/Systems/Photovoltaics/PVSystem/MaxPowerOutput",
    unit: None,
    description: "the PV system's or inverter's maximum power output, watts",
};

/// the <MechanicalVentilation> element the parser reads under its parent
pub static MECHANICAL_VENTILATION: HpxmlElement = HpxmlElement {
    name: "MechanicalVentilation",
    context: "BuildingDetails/Systems/MechanicalVentilation",
    unit: None,
    description: "the <MechanicalVentilation> element the parser reads under its parent",
};

/// the <Microwave> element the parser reads under its parent
pub static MICROWAVE: HpxmlElement = HpxmlElement {
    name: "Microwave",
    context: "Appliances (the microwave container; BEopt/OCHRE schedules only)",
    unit: None,
    description: "the <Microwave> element the parser reads under its parent",
};

/// the <MinCapacity> element the parser reads under its parent
pub static MIN_CAPACITY: HpxmlElement = HpxmlElement {
    name: "MinCapacity",
    context: "an AutosizingLimits element's minimum capacity (Btu/h)",
    unit: None,
    description: "the <MinCapacity> element the parser reads under its parent",
};

/// the <MinimumCapacity> element the parser reads under its parent
pub static MINIMUM_CAPACITY: HpxmlElement = HpxmlElement {
    name: "MinimumCapacity",
    context: "HeatPump (autosizing minimum)",
    unit: None,
    description: "the <MinimumCapacity> element the parser reads under its parent",
};

/// the <MiscLoads> element the parser reads under its parent
pub static MISC_LOADS: HpxmlElement = HpxmlElement {
    name: "MiscLoads",
    context: "BuildingDetails/MiscLoads",
    unit: None,
    description: "the <MiscLoads> element the parser reads under its parent",
};

/// the <ModuleType> element the parser reads under its parent
pub static MODULE_TYPE: HpxmlElement = HpxmlElement {
    name: "ModuleType",
    context: "BuildingDetails/Systems/Photovoltaics/PVSystem/ModuleType",
    unit: None,
    description: "the <ModuleType> element the parser reads under its parent",
};

/// the 12 monthly schedule multipliers (extension)
pub static MONTHLY_SCHEDULE_MULTIPLIERS: HpxmlElement = HpxmlElement {
    name: "MonthlyScheduleMultipliers",
    context: "the 12 monthly schedule multipliers (extension; a location prefix may name it)",
    unit: None,
    description: "the 12 monthly schedule multipliers (extension)",
};

/// the <Name> element the parser reads under its parent
pub static NAME: HpxmlElement = HpxmlElement {
    name: "Name",
    context: "the weather station's or element's Name child",
    unit: None,
    description: "the <Name> element the parser reads under its parent",
};

/// the <NaturalVentilation> element the parser reads under its parent
pub static NATURAL_VENTILATION: HpxmlElement = HpxmlElement {
    name: "NaturalVentilation",
    context: "an unavailable period's natural-ventilation kind",
    unit: None,
    description: "the <NaturalVentilation> element the parser reads under its parent",
};

/// the <NominalCapacity> element the parser reads under its parent
pub static NOMINAL_CAPACITY: HpxmlElement = HpxmlElement {
    name: "NominalCapacity",
    context: "BuildingDetails/Systems/Batteries/Battery/NominalCapacity | BuildingDetails/Systems/Vehicles/Vehicle/VehicleType/BatteryElectricVehicle/Battery/NominalCapacity",
    unit: None,
    description: "the <NominalCapacity> element the parser reads under its parent",
};

/// a layer's nominal R-value
pub static NOMINAL_R_VALUE: HpxmlElement = HpxmlElement {
    name: "NominalRValue",
    context: "BuildingDetails/Enclosure/Floors/Floor/Insulation/Layer/NominalRValue | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/Insulation/Layer/NominalRValue | BuildingDetails/Enclosure/Roofs/Roof/Insulation/Layer/NominalRValue | BuildingDetails/Enclosure/Slabs/Slab/ExteriorHorizontalInsulation/Layer/NominalRValue; and 3 more parent paths in the fixture corpus",
    unit: None,
    description: "a layer's nominal R-value",
};

/// the building's bedroom count (adjusted for the water heater's draw)
pub static NUMBEROF_BEDROOMS: HpxmlElement = HpxmlElement {
    name: "NumberofBedrooms",
    context: "BuildingDetails/BuildingSummary/BuildingConstruction/NumberofBedrooms",
    unit: None,
    description: "the building's bedroom count (adjusted for the water heater's draw)",
};

/// the <NumberofConditionedFloors> element the parser reads under its parent
pub static NUMBEROF_CONDITIONED_FLOORS: HpxmlElement = HpxmlElement {
    name: "NumberofConditionedFloors",
    context: "BuildingDetails/BuildingSummary/BuildingConstruction/NumberofConditionedFloors",
    unit: None,
    description: "the <NumberofConditionedFloors> element the parser reads under its parent",
};

/// the conditioned floors above grade
pub static NUMBEROF_CONDITIONED_FLOORS_ABOVE_GRADE: HpxmlElement = HpxmlElement {
    name: "NumberofConditionedFloorsAboveGrade",
    context: "BuildingDetails/BuildingSummary/BuildingConstruction/NumberofConditionedFloorsAboveGrade",
    unit: None,
    description: "the conditioned floors above grade",
};

/// the <NumberofResidents> element the parser reads under its parent
pub static NUMBEROF_RESIDENTS: HpxmlElement = HpxmlElement {
    name: "NumberofResidents",
    context: "BuildingDetails/BuildingSummary/BuildingOccupancy/NumberofResidents",
    unit: None,
    description: "the <NumberofResidents> element the parser reads under its parent",
};

/// the <PerformanceAdjustment> element the parser reads under its parent
pub static PERFORMANCE_ADJUSTMENT: HpxmlElement = HpxmlElement {
    name: "PerformanceAdjustment",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/PerformanceAdjustment",
    unit: None,
    description: "the <PerformanceAdjustment> element the parser reads under its parent",
};

/// the <Perimeter> element the parser reads under its parent
pub static PERIMETER: HpxmlElement = HpxmlElement {
    name: "Perimeter",
    context: "Slab",
    unit: None,
    description: "the <Perimeter> element the parser reads under its parent",
};

/// the <PerimeterInsulation> element the parser reads under its parent
pub static PERIMETER_INSULATION: HpxmlElement = HpxmlElement {
    name: "PerimeterInsulation",
    context: "BuildingDetails/Enclosure/Slabs/Slab/PerimeterInsulation",
    unit: None,
    description: "the <PerimeterInsulation> element the parser reads under its parent",
};

/// the <PermanentSpa> element the parser reads under its parent
pub static PERMANENT_SPA: HpxmlElement = HpxmlElement {
    name: "PermanentSpa",
    context: "Spas",
    unit: None,
    description: "the <PermanentSpa> element the parser reads under its parent",
};

/// the <Photovoltaics> element the parser reads under its parent
pub static PHOTOVOLTAICS: HpxmlElement = HpxmlElement {
    name: "Photovoltaics",
    context: "BuildingDetails/Systems/Photovoltaics",
    unit: None,
    description: "the <Photovoltaics> element the parser reads under its parent",
};

/// the <PilotPower> element the parser reads under its parent
pub static PILOT_POWER: HpxmlElement = HpxmlElement {
    name: "PilotPower",
    context: "WaterHeatingSystem (gas; the pilot light)",
    unit: None,
    description: "the <PilotPower> element the parser reads under its parent",
};

/// the <PipeInsulation> element the parser reads under its parent
pub static PIPE_INSULATION: HpxmlElement = HpxmlElement {
    name: "PipeInsulation",
    context: "BuildingDetails/Systems/WaterHeating/HotWaterDistribution/PipeInsulation",
    unit: None,
    description: "the <PipeInsulation> element the parser reads under its parent",
};

/// the <PipeRValue> element the parser reads under its parent
pub static PIPE_R_VALUE: HpxmlElement = HpxmlElement {
    name: "PipeRValue",
    context: "BuildingDetails/Systems/WaterHeating/HotWaterDistribution/PipeInsulation/PipeRValue",
    unit: None,
    description: "the <PipeRValue> element the parser reads under its parent",
};

/// the <PipingLength> element the parser reads under its parent
pub static PIPING_LENGTH: HpxmlElement = HpxmlElement {
    name: "PipingLength",
    context: "BuildingDetails/Systems/WaterHeating/HotWaterDistribution/SystemType/Standard/PipingLength",
    unit: None,
    description: "the <PipingLength> element the parser reads under its parent",
};

/// the <Pitch> element the parser reads under its parent
pub static PITCH: HpxmlElement = HpxmlElement {
    name: "Pitch",
    context: "BuildingDetails/Enclosure/Roofs/Roof/Pitch",
    unit: None,
    description: "the <Pitch> element the parser reads under its parent",
};

/// the <PlaceSettingCapacity> element the parser reads under its parent
pub static PLACE_SETTING_CAPACITY: HpxmlElement = HpxmlElement {
    name: "PlaceSettingCapacity",
    context: "the dishwasher's place-setting capacity",
    unit: None,
    description: "the <PlaceSettingCapacity> element the parser reads under its parent",
};

/// the <PlugLoad> element the parser reads under its parent
pub static PLUG_LOAD: HpxmlElement = HpxmlElement {
    name: "PlugLoad",
    context: "BuildingDetails/MiscLoads/PlugLoad",
    unit: None,
    description: "the <PlugLoad> element the parser reads under its parent",
};

/// the <PlugLoadType> element the parser reads under its parent
pub static PLUG_LOAD_TYPE: HpxmlElement = HpxmlElement {
    name: "PlugLoadType",
    context: "BuildingDetails/MiscLoads/PlugLoad/PlugLoadType",
    unit: None,
    description: "the <PlugLoadType> element the parser reads under its parent",
};

/// the <Pool> element the parser reads under its parent
pub static POOL: HpxmlElement = HpxmlElement {
    name: "Pool",
    context: "BuildingDetails/Pools/Pool",
    unit: None,
    description: "the <Pool> element the parser reads under its parent",
};

/// the <PoolPump> element the parser reads under its parent
pub static POOL_PUMP: HpxmlElement = HpxmlElement {
    name: "PoolPump",
    context: "BuildingDetails/Pools/Pool/PoolPumps/PoolPump",
    unit: None,
    description: "the <PoolPump> element the parser reads under its parent",
};

/// the <PoolPumps> element the parser reads under its parent
pub static POOL_PUMPS: HpxmlElement = HpxmlElement {
    name: "PoolPumps",
    context: "BuildingDetails/Pools/Pool/PoolPumps",
    unit: None,
    description: "the <PoolPumps> element the parser reads under its parent",
};

/// the <Pools> element the parser reads under its parent
pub static POOLS: HpxmlElement = HpxmlElement {
    name: "Pools",
    context: "BuildingDetails/Pools",
    unit: None,
    description: "the <Pools> element the parser reads under its parent",
};

/// the <PortableElectricSpa> element the parser reads under its parent
pub static PORTABLE_ELECTRIC_SPA: HpxmlElement = HpxmlElement {
    name: "PortableElectricSpa",
    context: "Spas",
    unit: None,
    description: "the <PortableElectricSpa> element the parser reads under its parent",
};

/// the <PrimaryCoolingSystem> element the parser reads under its parent
pub static PRIMARY_COOLING_SYSTEM: HpxmlElement = HpxmlElement {
    name: "PrimaryCoolingSystem",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/PrimarySystems/PrimaryCoolingSystem",
    unit: None,
    description: "the <PrimaryCoolingSystem> element the parser reads under its parent",
};

/// the <PrimaryHeatingSystem> element the parser reads under its parent
pub static PRIMARY_HEATING_SYSTEM: HpxmlElement = HpxmlElement {
    name: "PrimaryHeatingSystem",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/PrimarySystems/PrimaryHeatingSystem",
    unit: None,
    description: "the <PrimaryHeatingSystem> element the parser reads under its parent",
};

/// the <PrimaryIndicator> element the parser reads under its parent
pub static PRIMARY_INDICATOR: HpxmlElement = HpxmlElement {
    name: "PrimaryIndicator",
    context: "BuildingDetails/Appliances/Refrigerator/PrimaryIndicator",
    unit: None,
    description: "the <PrimaryIndicator> element the parser reads under its parent",
};

/// the <PrimarySystems> element the parser reads under its parent
pub static PRIMARY_SYSTEMS: HpxmlElement = HpxmlElement {
    name: "PrimarySystems",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/PrimarySystems",
    unit: None,
    description: "the <PrimarySystems> element the parser reads under its parent",
};

/// the <PVSystem> element the parser reads under its parent
pub static PV_SYSTEM: HpxmlElement = HpxmlElement {
    name: "PVSystem",
    context: "BuildingDetails/Systems/Photovoltaics/PVSystem",
    unit: None,
    description: "the <PVSystem> element the parser reads under its parent",
};

/// the <RadiantBarrier> element the parser reads under its parent
pub static RADIANT_BARRIER: HpxmlElement = HpxmlElement {
    name: "RadiantBarrier",
    context: "BuildingDetails/Enclosure/Floors/Floor/RadiantBarrier | BuildingDetails/Enclosure/Roofs/Roof/RadiantBarrier | BuildingDetails/Enclosure/Walls/Wall/RadiantBarrier",
    unit: None,
    description: "the <RadiantBarrier> element the parser reads under its parent",
};

/// the <RatedAnnualkWh> element the parser reads under its parent
pub static RATED_ANNUALK_WH: HpxmlElement = HpxmlElement {
    name: "RatedAnnualkWh",
    context: "the appliance's rated annual energy (kWh/year)",
    unit: None,
    description: "the <RatedAnnualkWh> element the parser reads under its parent",
};

/// the <RatedFlowRate> element the parser reads under its parent
pub static RATED_FLOW_RATE: HpxmlElement = HpxmlElement {
    name: "RatedFlowRate",
    context: "BuildingDetails/Systems/MechanicalVentilation/VentilationFans/VentilationFan/RatedFlowRate",
    unit: None,
    description: "the <RatedFlowRate> element the parser reads under its parent",
};

/// the <RatedPowerOutput> element the parser reads under its parent
pub static RATED_POWER_OUTPUT: HpxmlElement = HpxmlElement {
    name: "RatedPowerOutput",
    context: "BuildingDetails/Systems/Batteries/Battery/RatedPowerOutput",
    unit: None,
    description: "the <RatedPowerOutput> element the parser reads under its parent",
};

/// the <Recirculation> element the parser reads under its parent
pub static RECIRCULATION: HpxmlElement = HpxmlElement {
    name: "Recirculation",
    context: "WaterHeatingSystem/HotWaterDistribution",
    unit: None,
    description: "the <Recirculation> element the parser reads under its parent",
};

/// the <RecoveryEfficiency> element the parser reads under its parent
pub static RECOVERY_EFFICIENCY: HpxmlElement = HpxmlElement {
    name: "RecoveryEfficiency",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/RecoveryEfficiency",
    unit: None,
    description: "the <RecoveryEfficiency> element the parser reads under its parent",
};

/// the <Refrigerator> element the parser reads under its parent
pub static REFRIGERATOR: HpxmlElement = HpxmlElement {
    name: "Refrigerator",
    context: "BuildingDetails/Appliances/Refrigerator",
    unit: None,
    description: "the <Refrigerator> element the parser reads under its parent",
};

/// the <RelatedHVACSystem> element the parser reads under its parent
pub static RELATED_HVAC_SYSTEM: HpxmlElement = HpxmlElement {
    name: "RelatedHVACSystem",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/RelatedHVACSystem",
    unit: None,
    description: "the <RelatedHVACSystem> element the parser reads under its parent",
};

/// the <ResidentialFacilityType> element the parser reads under its parent
pub static RESIDENTIAL_FACILITY_TYPE: HpxmlElement = HpxmlElement {
    name: "ResidentialFacilityType",
    context: "BuildingDetails/BuildingSummary/BuildingConstruction/ResidentialFacilityType",
    unit: None,
    description: "the <ResidentialFacilityType> element the parser reads under its parent",
};

/// the <RimJoist> element the parser reads under its parent
pub static RIM_JOIST: HpxmlElement = HpxmlElement {
    name: "RimJoist",
    context: "Enclosure/RimJoists (one rim joist)",
    unit: None,
    description: "the <RimJoist> element the parser reads under its parent",
};

/// the <RimJoists> element the parser reads under its parent
pub static RIM_JOISTS: HpxmlElement = HpxmlElement {
    name: "RimJoists",
    context: "Enclosure (the rim-joist container)",
    unit: None,
    description: "the <RimJoists> element the parser reads under its parent",
};

/// the <Roof> element the parser reads under its parent
pub static ROOF: HpxmlElement = HpxmlElement {
    name: "Roof",
    context: "Enclosure/Roofs (one roof)",
    unit: None,
    description: "the <Roof> element the parser reads under its parent",
};

/// the <Roofs> element the parser reads under its parent
pub static ROOFS: HpxmlElement = HpxmlElement {
    name: "Roofs",
    context: "Enclosure (the roof container)",
    unit: None,
    description: "the <Roofs> element the parser reads under its parent",
};

/// the <RoofType> element the parser reads under its parent
pub static ROOF_TYPE: HpxmlElement = HpxmlElement {
    name: "RoofType",
    context: "BuildingDetails/Enclosure/Roofs/Roof/RoofType",
    unit: None,
    description: "the <RoofType> element the parser reads under its parent",
};

/// the <RoundTripEfficiency> element the parser reads under its parent
pub static ROUND_TRIP_EFFICIENCY: HpxmlElement = HpxmlElement {
    name: "RoundTripEfficiency",
    context: "Systems/Batteries/Battery",
    unit: None,
    description: "the <RoundTripEfficiency> element the parser reads under its parent",
};

/// the <RValue> element the parser reads under its parent
pub static R_VALUE: HpxmlElement = HpxmlElement {
    name: "RValue",
    context: "BuildingDetails/Enclosure/Doors/Door/RValue",
    unit: None,
    description: "the <RValue> element the parser reads under its parent",
};

/// the <SEER> element the parser reads under its parent
pub static SEER: HpxmlElement = HpxmlElement {
    name: "SEER",
    context: "a cooling system's SEER (the legacy direct-child form)",
    unit: None,
    description: "the <SEER> element the parser reads under its parent",
};

/// the <SEER2> element the parser reads under its parent
pub static SEER_2: HpxmlElement = HpxmlElement {
    name: "SEER2",
    context: "a cooling system's SEER2 (the legacy direct-child form)",
    unit: None,
    description: "the <SEER2> element the parser reads under its parent",
};

/// the <sensible_gain_fraction> element the parser reads under its parent
pub static SENSIBLE_GAIN_FRACTION: HpxmlElement = HpxmlElement {
    name: "sensible_gain_fraction",
    context: "the sensible gain fraction (the synthetic extension element)",
    unit: None,
    description: "the <sensible_gain_fraction> element the parser reads under its parent",
};

/// the <SensibleHeatFraction> element the parser reads under its parent
pub static SENSIBLE_HEAT_FRACTION: HpxmlElement = HpxmlElement {
    name: "SensibleHeatFraction",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/SensibleHeatFraction",
    unit: None,
    description: "the <SensibleHeatFraction> element the parser reads under its parent",
};

/// the <SensibleRecoveryEfficiency> element the parser reads under its parent
pub static SENSIBLE_RECOVERY_EFFICIENCY: HpxmlElement = HpxmlElement {
    name: "SensibleRecoveryEfficiency",
    context: "VentilationFan (HRV/ERV)",
    unit: None,
    description: "the <SensibleRecoveryEfficiency> element the parser reads under its parent",
};

/// the heating setback's start hour (extension)
pub static SETBACK_START_HOUR_HEATING: HpxmlElement = HpxmlElement {
    name: "SetbackStartHourHeating",
    context: "HVACControl/extension (the heating setback's start hour)",
    unit: None,
    description: "the heating setback's start hour (extension)",
};

/// the heating setback temperature, degrees Fahrenheit
pub static SETBACK_TEMP_HEATING_SEASON: HpxmlElement = HpxmlElement {
    name: "SetbackTempHeatingSeason",
    context: "HVACControl (the heating setback temperature, degrees Fahrenheit)",
    unit: None,
    description: "the heating setback temperature, degrees Fahrenheit",
};

/// the constant cooling setpoint, degrees Fahrenheit
pub static SETPOINT_TEMP_COOLING_SEASON: HpxmlElement = HpxmlElement {
    name: "SetpointTempCoolingSeason",
    context: "BuildingDetails/Systems/HVAC/HVACControl/SetpointTempCoolingSeason",
    unit: None,
    description: "the constant cooling setpoint, degrees Fahrenheit",
};

/// the constant heating setpoint, degrees Fahrenheit
pub static SETPOINT_TEMP_HEATING_SEASON: HpxmlElement = HpxmlElement {
    name: "SetpointTempHeatingSeason",
    context: "BuildingDetails/Systems/HVAC/HVACControl/SetpointTempHeatingSeason",
    unit: None,
    description: "the constant heating setpoint, degrees Fahrenheit",
};

/// the cooling setup's start hour (extension)
pub static SETUP_START_HOUR_COOLING: HpxmlElement = HpxmlElement {
    name: "SetupStartHourCooling",
    context: "HVACControl/extension (the cooling setup's start hour)",
    unit: None,
    description: "the cooling setup's start hour (extension)",
};

/// the cooling setup temperature, degrees Fahrenheit
pub static SETUP_TEMP_COOLING_SEASON: HpxmlElement = HpxmlElement {
    name: "SetupTempCoolingSeason",
    context: "HVACControl (the cooling setup temperature, degrees Fahrenheit)",
    unit: None,
    description: "the cooling setup temperature, degrees Fahrenheit",
};

/// the <SHGC> element the parser reads under its parent
pub static SHGC: HpxmlElement = HpxmlElement {
    name: "SHGC",
    context: "BuildingDetails/Enclosure/Windows/Window/SHGC",
    unit: None,
    description: "the <SHGC> element the parser reads under its parent",
};

/// the <ShieldingofHome> element the parser reads under its parent
pub static SHIELDINGOF_HOME: HpxmlElement = HpxmlElement {
    name: "ShieldingofHome",
    context: "BuildingDetails/BuildingSummary/Site/ShieldingofHome",
    unit: None,
    description: "the <ShieldingofHome> element the parser reads under its parent",
};

/// the <Siding> element the parser reads under its parent
pub static SIDING: HpxmlElement = HpxmlElement {
    name: "Siding",
    context: "a wall's or rim joist's siding finish (its Type element)",
    unit: None,
    description: "the <Siding> element the parser reads under its parent",
};

/// the <SimulationControl> element the parser reads under its parent
pub static SIMULATION_CONTROL: HpxmlElement = HpxmlElement {
    name: "SimulationControl",
    context: "SoftwareInfo/extension/SimulationControl (the simulation control's extension)",
    unit: None,
    description: "the <SimulationControl> element the parser reads under its parent",
};

/// the <Site> element the parser reads under its parent
pub static SITE: HpxmlElement = HpxmlElement {
    name: "Site",
    context: "BuildingDetails/BuildingSummary/Site",
    unit: None,
    description: "the <Site> element the parser reads under its parent",
};

/// the <SiteType> element the parser reads under its parent
pub static SITE_TYPE: HpxmlElement = HpxmlElement {
    name: "SiteType",
    context: "Building/Site (the site's type)",
    unit: None,
    description: "the <SiteType> element the parser reads under its parent",
};

/// the <Skylight> element the parser reads under its parent
pub static SKYLIGHT: HpxmlElement = HpxmlElement {
    name: "Skylight",
    context: "Enclosure/Skylights",
    unit: None,
    description: "the <Skylight> element the parser reads under its parent",
};

/// the <Skylights> element the parser reads under its parent
pub static SKYLIGHTS: HpxmlElement = HpxmlElement {
    name: "Skylights",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/extension/DesignLoads/CoolingSensible/Skylights | BuildingDetails/Systems/HVAC/HVACPlant/extension/DesignLoads/Heating/Skylights",
    unit: None,
    description: "the <Skylights> element the parser reads under its parent",
};

/// the <Slab> element the parser reads under its parent
pub static SLAB: HpxmlElement = HpxmlElement {
    name: "Slab",
    context: "Enclosure/Slabs (one slab)",
    unit: None,
    description: "the <Slab> element the parser reads under its parent",
};

/// the <Slabs> element the parser reads under its parent
pub static SLABS: HpxmlElement = HpxmlElement {
    name: "Slabs",
    context: "Enclosure (the slab container)",
    unit: None,
    description: "the <Slabs> element the parser reads under its parent",
};

/// the <SlabOnGrade> element the parser reads under its parent
pub static SLAB_ON_GRADE: HpxmlElement = HpxmlElement {
    name: "SlabOnGrade",
    context: "BuildingDetails/Enclosure/Foundations/Foundation/FoundationType/SlabOnGrade",
    unit: None,
    description: "the <SlabOnGrade> element the parser reads under its parent",
};

/// the <SoftwareInfo> element the parser reads under its parent
pub static SOFTWARE_INFO: HpxmlElement = HpxmlElement {
    name: "SoftwareInfo",
    context: "HPXML root (the SoftwareInfo extension tree)",
    unit: None,
    description: "the <SoftwareInfo> element the parser reads under its parent",
};

/// the <SolarAbsorptance> element the parser reads under its parent
pub static SOLAR_ABSORPTANCE: HpxmlElement = HpxmlElement {
    name: "SolarAbsorptance",
    context: "BuildingDetails/Enclosure/RimJoists/RimJoist/SolarAbsorptance | BuildingDetails/Enclosure/Roofs/Roof/SolarAbsorptance | BuildingDetails/Enclosure/Walls/Wall/SolarAbsorptance",
    unit: None,
    description: "the <SolarAbsorptance> element the parser reads under its parent",
};

/// the <Spas> element the parser reads under its parent
pub static SPAS: HpxmlElement = HpxmlElement {
    name: "Spas",
    context: "BuildingDetails",
    unit: None,
    description: "the <Spas> element the parser reads under its parent",
};

/// the <SpecificHeat> element the parser reads under its parent
pub static SPECIFIC_HEAT: HpxmlElement = HpxmlElement {
    name: "SpecificHeat",
    context: "material Layer (mass layers)",
    unit: None,
    description: "the <SpecificHeat> element the parser reads under its parent",
};

/// the <Standard> element the parser reads under its parent
pub static STANDARD: HpxmlElement = HpxmlElement {
    name: "Standard",
    context: "BuildingDetails/Systems/WaterHeating/HotWaterDistribution/SystemType/Standard",
    unit: None,
    description: "the <Standard> element the parser reads under its parent",
};

/// the <StandbyPower> element the parser reads under its parent
pub static STANDBY_POWER: HpxmlElement = HpxmlElement {
    name: "StandbyPower",
    context: "PermanentSpa and PortableElectricSpa",
    unit: None,
    description: "the <StandbyPower> element the parser reads under its parent",
};

/// the <StudSpacing> element the parser reads under its parent
pub static STUD_SPACING: HpxmlElement = HpxmlElement {
    name: "StudSpacing",
    context: "Wall (the framing stud spacing)",
    unit: None,
    description: "the <StudSpacing> element the parser reads under its parent",
};

/// the <StudWidth> element the parser reads under its parent
pub static STUD_WIDTH: HpxmlElement = HpxmlElement {
    name: "StudWidth",
    context: "Wall (the framing stud width)",
    unit: None,
    description: "the <StudWidth> element the parser reads under its parent",
};

/// the <SummerShadingCoefficient> element the parser reads under its parent
pub static SUMMER_SHADING_COEFFICIENT: HpxmlElement = HpxmlElement {
    name: "SummerShadingCoefficient",
    context: "BuildingDetails/Enclosure/Windows/Window/ExteriorShading/SummerShadingCoefficient | BuildingDetails/Enclosure/Windows/Window/InteriorShading/SummerShadingCoefficient",
    unit: None,
    description: "the <SummerShadingCoefficient> element the parser reads under its parent",
};

/// the <SupplementalHeatingLockoutTemperature> element the parser reads under its parent
pub static SUPPLEMENTAL_HEATING_LOCKOUT_TEMPERATURE: HpxmlElement = HpxmlElement {
    name: "SupplementalHeatingLockoutTemperature",
    context: "HeatPump",
    unit: None,
    description: "the <SupplementalHeatingLockoutTemperature> element the parser reads under its parent",
};

/// the system's or load's identifier; the parser reads its @id
pub static SYSTEM_IDENTIFIER: HpxmlElement = HpxmlElement {
    name: "SystemIdentifier",
    context: "BuildingDetails/Appliances/ClothesDryer/SystemIdentifier | BuildingDetails/Appliances/ClothesWasher/SystemIdentifier | BuildingDetails/Appliances/CookingRange/SystemIdentifier | BuildingDetails/Appliances/Dehumidifier/SystemIdentifier; and 56 more parent paths in the fixture corpus",
    unit: None,
    description: "the system's or load's identifier; the parser reads its @id",
};

/// the <SystemLossesFraction> element the parser reads under its parent
pub static SYSTEM_LOSSES_FRACTION: HpxmlElement = HpxmlElement {
    name: "SystemLossesFraction",
    context: "BuildingDetails/Systems/Photovoltaics/PVSystem/SystemLossesFraction",
    unit: None,
    description: "the <SystemLossesFraction> element the parser reads under its parent",
};

/// the <Systems> element the parser reads under its parent
pub static SYSTEMS: HpxmlElement = HpxmlElement {
    name: "Systems",
    context: "BuildingDetails/Systems",
    unit: None,
    description: "the <Systems> element the parser reads under its parent",
};

/// the <SystemType> element the parser reads under its parent
pub static SYSTEM_TYPE: HpxmlElement = HpxmlElement {
    name: "SystemType",
    context: "BuildingDetails/Systems/SolarThermal/SolarThermalSystem/SystemType | BuildingDetails/Systems/WaterHeating/HotWaterDistribution/SystemType",
    unit: None,
    description: "the <SystemType> element the parser reads under its parent",
};

/// the <TankHeight> element the parser reads under its parent
pub static TANK_HEIGHT: HpxmlElement = HpxmlElement {
    name: "TankHeight",
    context: "WaterHeatingSystem (storage tanks)",
    unit: None,
    description: "the <TankHeight> element the parser reads under its parent",
};

/// the <TankVolume> element the parser reads under its parent
pub static TANK_VOLUME: HpxmlElement = HpxmlElement {
    name: "TankVolume",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/TankVolume",
    unit: None,
    description: "the <TankVolume> element the parser reads under its parent",
};

/// the <Temperature> element the parser reads under its parent
pub static TEMPERATURE: HpxmlElement = HpxmlElement {
    name: "Temperature",
    context: "BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/extension/HeatingCapacityRetention/Temperature",
    unit: None,
    description: "the <Temperature> element the parser reads under its parent",
};

/// the zone air capacitance multiplier (an advanced research feature)
pub static TEMPERATURE_CAPACITANCE_MULTIPLIER: HpxmlElement = HpxmlElement {
    name: "TemperatureCapacitanceMultiplier",
    context: "the zone air temperature capacitance multiplier (an advanced research feature)",
    unit: None,
    description: "the zone air capacitance multiplier (an advanced research feature)",
};

/// the <text> element the parser reads under its parent
pub static TEXT: HpxmlElement = HpxmlElement {
    name: "text",
    context: "the XML-1.0 normalisation pin's synthetic document",
    unit: None,
    description: "the <text> element the parser reads under its parent",
};

/// an insulation or finish layer's thickness
pub static THICKNESS: HpxmlElement = HpxmlElement {
    name: "Thickness",
    context: "BuildingDetails/Enclosure/Floors/Floor/InteriorFinish/Thickness | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/InteriorFinish/Thickness | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/Thickness | BuildingDetails/Enclosure/Roofs/Roof/InteriorFinish/Thickness; and 3 more parent paths in the fixture corpus",
    unit: None,
    description: "an insulation or finish layer's thickness",
};

/// the <Tilt> element the parser reads under its parent
pub static TILT: HpxmlElement = HpxmlElement {
    name: "Tilt",
    context: "a surface's tilt from horizontal, degrees",
    unit: None,
    description: "the <Tilt> element the parser reads under its parent",
};

/// the site's time zone wrapper
pub static TIME_ZONE: HpxmlElement = HpxmlElement {
    name: "TimeZone",
    context: "Building/Site",
    unit: None,
    description: "the site's time zone wrapper",
};

/// the <TotalAnnualPowerConsumptionInStandbyMode> element the parser reads under its parent
pub static TOTAL_ANNUAL_POWER_CONSUMPTION_IN_STANDBY_MODE: HpxmlElement = HpxmlElement {
    name: "TotalAnnualPowerConsumptionInStandbyMode",
    context: "PermanentSpa and PortableElectricSpa",
    unit: None,
    description: "the <TotalAnnualPowerConsumptionInStandbyMode> element the parser reads under its parent",
};

/// the <TotalRecoveryEfficiency> element the parser reads under its parent
pub static TOTAL_RECOVERY_EFFICIENCY: HpxmlElement = HpxmlElement {
    name: "TotalRecoveryEfficiency",
    context: "VentilationFan (HRV/ERV)",
    unit: None,
    description: "the <TotalRecoveryEfficiency> element the parser reads under its parent",
};

/// the heating setback's hours per week
pub static TOTAL_SETBACK_HOURSPER_WEEK_HEATING: HpxmlElement = HpxmlElement {
    name: "TotalSetbackHoursperWeekHeating",
    context: "HVACControl (the heating setback's hours per week)",
    unit: None,
    description: "the heating setback's hours per week",
};

/// the cooling setup's hours per week
pub static TOTAL_SETUP_HOURSPER_WEEK_COOLING: HpxmlElement = HpxmlElement {
    name: "TotalSetupHoursperWeekCooling",
    context: "HVACControl (the cooling setup's hours per week)",
    unit: None,
    description: "the cooling setup's hours per week",
};

/// the <Tracking> element the parser reads under its parent
pub static TRACKING: HpxmlElement = HpxmlElement {
    name: "Tracking",
    context: "BuildingDetails/Systems/Photovoltaics/PVSystem/Tracking",
    unit: None,
    description: "the <Tracking> element the parser reads under its parent",
};

/// a discriminator element (attic type, foundation type, finish type)
pub static TYPE: HpxmlElement = HpxmlElement {
    name: "Type",
    context: "BuildingDetails/Appliances/Dehumidifier/Type | BuildingDetails/Enclosure/Floors/Floor/InteriorFinish/Type | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/InteriorFinish/Type | BuildingDetails/Enclosure/FoundationWalls/FoundationWall/Type; and 13 more parent paths in the fixture corpus",
    unit: None,
    description: "a discriminator element (attic type, foundation type, finish type)",
};

/// the <TypeOfInfiltrationLeakage> element the parser reads under its parent
pub static TYPE_OF_INFILTRATION_LEAKAGE: HpxmlElement = HpxmlElement {
    name: "TypeOfInfiltrationLeakage",
    context: "BuildingDetails/Enclosure/AirInfiltration/AirInfiltrationMeasurement/TypeOfInfiltrationLeakage",
    unit: None,
    description: "the <TypeOfInfiltrationLeakage> element the parser reads under its parent",
};

/// the <UFactor> element the parser reads under its parent
pub static U_FACTOR: HpxmlElement = HpxmlElement {
    name: "UFactor",
    context: "BuildingDetails/Enclosure/Windows/Window/UFactor",
    unit: None,
    description: "the <UFactor> element the parser reads under its parent",
};

/// one unavailable period whose steps zero the schedule columns
pub static UNAVAILABLE_PERIOD: HpxmlElement = HpxmlElement {
    name: "UnavailablePeriod",
    context: "one unavailable period (its flagged steps zero the schedule columns)",
    unit: None,
    description: "one unavailable period whose steps zero the schedule columns",
};

/// the <UnavailablePeriods> element the parser reads under its parent
pub static UNAVAILABLE_PERIODS: HpxmlElement = HpxmlElement {
    name: "UnavailablePeriods",
    context: "SoftwareInfo/extension (the unavailable periods' container)",
    unit: None,
    description: "the <UnavailablePeriods> element the parser reads under its parent",
};

/// the <UnderSlabInsulation> element the parser reads under its parent
pub static UNDER_SLAB_INSULATION: HpxmlElement = HpxmlElement {
    name: "UnderSlabInsulation",
    context: "BuildingDetails/Enclosure/Slabs/Slab/UnderSlabInsulation",
    unit: None,
    description: "the <UnderSlabInsulation> element the parser reads under its parent",
};

/// the <UniformEnergyFactor> element the parser reads under its parent
pub static UNIFORM_ENERGY_FACTOR: HpxmlElement = HpxmlElement {
    name: "UniformEnergyFactor",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/UniformEnergyFactor",
    unit: None,
    description: "the <UniformEnergyFactor> element the parser reads under its parent",
};

/// the <UnitHeightAboveGrade> element the parser reads under its parent
pub static UNIT_HEIGHT_ABOVE_GRADE: HpxmlElement = HpxmlElement {
    name: "UnitHeightAboveGrade",
    context: "BuildingDetails/BuildingSummary/BuildingConstruction/UnitHeightAboveGrade",
    unit: None,
    description: "the <UnitHeightAboveGrade> element the parser reads under its parent",
};

/// the <UnitofMeasure> element the parser reads under its parent
pub static UNITOF_MEASURE: HpxmlElement = HpxmlElement {
    name: "UnitofMeasure",
    context: "BuildingDetails/Enclosure/AirInfiltration/AirInfiltrationMeasurement/BuildingAirLeakage/UnitofMeasure | BuildingDetails/Enclosure/Attics/Attic/VentilationRate/UnitofMeasure | BuildingDetails/Enclosure/Foundations/Foundation/VentilationRate/UnitofMeasure",
    unit: None,
    description: "the <UnitofMeasure> element the parser reads under its parent",
};

/// the units attribute-carrier of a Load, Usage or Value element
pub static UNITS: HpxmlElement = HpxmlElement {
    name: "Units",
    context: "BuildingDetails/HotTubs/HotTub/Heater/Load/Units | BuildingDetails/HotTubs/HotTub/HotTubPumps/HotTubPump/Load/Units | BuildingDetails/Lighting/LightingGroup/Load/Units | BuildingDetails/MiscLoads/FuelLoad/Load/Units; and 16 more parent paths in the fixture corpus",
    unit: None,
    description: "the units attribute-carrier of a Load, Usage or Value element",
};

/// the <Usage> element the parser reads under its parent
pub static USAGE: HpxmlElement = HpxmlElement {
    name: "Usage",
    context: "xml_helpers' energy-element tests' synthetic Load (no fixture carries it)",
    unit: None,
    description: "the <Usage> element the parser reads under its parent",
};

/// the extension's usage multiplier scaling the load
pub static USAGE_MULTIPLIER: HpxmlElement = HpxmlElement {
    name: "UsageMultiplier",
    context: "BuildingDetails/Appliances/ClothesDryer/extension/UsageMultiplier | BuildingDetails/Appliances/ClothesWasher/extension/UsageMultiplier | BuildingDetails/Appliances/CookingRange/extension/UsageMultiplier | BuildingDetails/Appliances/Dishwasher/extension/UsageMultiplier; and 5 more parent paths in the fixture corpus",
    unit: None,
    description: "the extension's usage multiplier scaling the load",
};

/// the <UsedForSeasonalCoolingLoadReduction> element the parser reads under its parent
pub static USED_FOR_SEASONAL_COOLING_LOAD_REDUCTION: HpxmlElement = HpxmlElement {
    name: "UsedForSeasonalCoolingLoadReduction",
    context: "the attic's seasonal-cooling load reduction flag (an xs:boolean)",
    unit: None,
    description: "the <UsedForSeasonalCoolingLoadReduction> element the parser reads under its parent",
};

/// the <UsedForWholeBuildingVentilation> element the parser reads under its parent
pub static USED_FOR_WHOLE_BUILDING_VENTILATION: HpxmlElement = HpxmlElement {
    name: "UsedForWholeBuildingVentilation",
    context: "BuildingDetails/Systems/MechanicalVentilation/VentilationFans/VentilationFan/UsedForWholeBuildingVentilation",
    unit: None,
    description: "the <UsedForWholeBuildingVentilation> element the parser reads under its parent",
};

/// the site's standard-time UTC offset, hours
pub static UTC_OFFSET: HpxmlElement = HpxmlElement {
    name: "UTCOffset",
    context: "Building/Site/TimeZone",
    unit: None,
    description: "the site's standard-time UTC offset, hours",
};

/// the numeric value of a Load, Usage or units-carried element
pub static VALUE: HpxmlElement = HpxmlElement {
    name: "Value",
    context: "BuildingDetails/Enclosure/Attics/Attic/VentilationRate/Value | BuildingDetails/Enclosure/Foundations/Foundation/VentilationRate/Value | BuildingDetails/HotTubs/HotTub/Heater/Load/Value | BuildingDetails/HotTubs/HotTub/HotTubPumps/HotTubPump/Load/Value; and 18 more parent paths in the fixture corpus",
    unit: None,
    description: "the numeric value of a Load, Usage or units-carried element",
};

/// the <Vented> element the parser reads under its parent
pub static VENTED: HpxmlElement = HpxmlElement {
    name: "Vented",
    context: "BuildingDetails/Appliances/ClothesDryer/Vented | BuildingDetails/Enclosure/Attics/Attic/AtticType/Attic/Vented | BuildingDetails/Enclosure/Foundations/Foundation/FoundationType/Crawlspace/Vented",
    unit: None,
    description: "the <Vented> element the parser reads under its parent",
};

/// the <VentilationFan> element the parser reads under its parent
pub static VENTILATION_FAN: HpxmlElement = HpxmlElement {
    name: "VentilationFan",
    context: "BuildingDetails/Systems/MechanicalVentilation/VentilationFans/VentilationFan",
    unit: None,
    description: "the <VentilationFan> element the parser reads under its parent",
};

/// the <VentilationFans> element the parser reads under its parent
pub static VENTILATION_FANS: HpxmlElement = HpxmlElement {
    name: "VentilationFans",
    context: "BuildingDetails/Systems/MechanicalVentilation/VentilationFans",
    unit: None,
    description: "the <VentilationFans> element the parser reads under its parent",
};

/// the <VentilationRate> element the parser reads under its parent
pub static VENTILATION_RATE: HpxmlElement = HpxmlElement {
    name: "VentilationRate",
    context: "BuildingDetails/Enclosure/Attics/Attic/VentilationRate | BuildingDetails/Enclosure/Foundations/Foundation/VentilationRate",
    unit: None,
    description: "the <VentilationRate> element the parser reads under its parent",
};

/// the <Version> element the parser reads under its parent
pub static VERSION: HpxmlElement = HpxmlElement {
    name: "Version",
    context: "SoftwareInfo/extension/ERICalculation (the ANSI/RESNET/ICC 301 version)",
    unit: None,
    description: "the <Version> element the parser reads under its parent",
};

/// the <Wall> element the parser reads under its parent
pub static WALL: HpxmlElement = HpxmlElement {
    name: "Wall",
    context: "Enclosure/Walls (one wall)",
    unit: None,
    description: "the <Wall> element the parser reads under its parent",
};

/// the <WallHeight> element the parser reads under its parent
pub static WALL_HEIGHT: HpxmlElement = HpxmlElement {
    name: "WallHeight",
    context: "Wall and FoundationWall (the wall height)",
    unit: None,
    description: "the <WallHeight> element the parser reads under its parent",
};

/// the <Walls> element the parser reads under its parent
pub static WALLS: HpxmlElement = HpxmlElement {
    name: "Walls",
    context: "Enclosure (the wall container)",
    unit: None,
    description: "the <Walls> element the parser reads under its parent",
};

/// the <WallType> element the parser reads under its parent
pub static WALL_TYPE: HpxmlElement = HpxmlElement {
    name: "WallType",
    context: "BuildingDetails/Enclosure/Walls/Wall/WallType",
    unit: None,
    description: "the <WallType> element the parser reads under its parent",
};

/// the <WaterFixture> element the parser reads under its parent
pub static WATER_FIXTURE: HpxmlElement = HpxmlElement {
    name: "WaterFixture",
    context: "BuildingDetails/Systems/WaterHeating/WaterFixture",
    unit: None,
    description: "the <WaterFixture> element the parser reads under its parent",
};

/// the <WaterFixturesUsageMultiplier> element the parser reads under its parent
pub static WATER_FIXTURES_USAGE_MULTIPLIER: HpxmlElement = HpxmlElement {
    name: "WaterFixturesUsageMultiplier",
    context: "BuildingDetails/Systems/WaterHeating/extension/WaterFixturesUsageMultiplier",
    unit: None,
    description: "the <WaterFixturesUsageMultiplier> element the parser reads under its parent",
};

/// the <WaterHeaterAutosizingFactor> element the parser reads under its parent
pub static WATER_HEATER_AUTOSIZING_FACTOR: HpxmlElement = HpxmlElement {
    name: "WaterHeaterAutosizingFactor",
    context: "WaterHeatingSystem/extension",
    unit: None,
    description: "the <WaterHeaterAutosizingFactor> element the parser reads under its parent",
};

/// the <WaterHeaterInsulation> element the parser reads under its parent
pub static WATER_HEATER_INSULATION: HpxmlElement = HpxmlElement {
    name: "WaterHeaterInsulation",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/WaterHeaterInsulation",
    unit: None,
    description: "the <WaterHeaterInsulation> element the parser reads under its parent",
};

/// the <WaterHeaterType> element the parser reads under its parent
pub static WATER_HEATER_TYPE: HpxmlElement = HpxmlElement {
    name: "WaterHeaterType",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/WaterHeaterType",
    unit: None,
    description: "the <WaterHeaterType> element the parser reads under its parent",
};

/// the <WaterHeating> element the parser reads under its parent
pub static WATER_HEATING: HpxmlElement = HpxmlElement {
    name: "WaterHeating",
    context: "BuildingDetails/Systems/WaterHeating",
    unit: None,
    description: "the <WaterHeating> element the parser reads under its parent",
};

/// the <WaterHeatingSystem> element the parser reads under its parent
pub static WATER_HEATING_SYSTEM: HpxmlElement = HpxmlElement {
    name: "WaterHeatingSystem",
    context: "BuildingDetails/Systems/WaterHeating/WaterHeatingSystem",
    unit: None,
    description: "the <WaterHeatingSystem> element the parser reads under its parent",
};

/// the <WeatherStation> element the parser reads under its parent
pub static WEATHER_STATION: HpxmlElement = HpxmlElement {
    name: "WeatherStation",
    context: "the HPXML weather-station element (the resstock reader's name lookup)",
    unit: None,
    description: "the <WeatherStation> element the parser reads under its parent",
};

/// the 24-hour weekday schedule fractions (extension)
pub static WEEKDAY_SCHEDULE_FRACTIONS: HpxmlElement = HpxmlElement {
    name: "WeekdayScheduleFractions",
    context: "the 24-hour weekday schedule fractions (extension; a location prefix may name it)",
    unit: None,
    description: "the 24-hour weekday schedule fractions (extension)",
};

/// the 24-hour weekday cooling setpoint profile, degrees Fahrenheit
pub static WEEKDAY_SETPOINT_TEMPS_COOLING_SEASON: HpxmlElement = HpxmlElement {
    name: "WeekdaySetpointTempsCoolingSeason",
    context: "HVACControl/extension (the 24-hour weekday cooling profile)",
    unit: None,
    description: "the 24-hour weekday cooling setpoint profile, degrees Fahrenheit",
};

/// the 24-hour weekday heating setpoint profile, degrees Fahrenheit
pub static WEEKDAY_SETPOINT_TEMPS_HEATING_SEASON: HpxmlElement = HpxmlElement {
    name: "WeekdaySetpointTempsHeatingSeason",
    context: "HVACControl/extension (the 24-hour weekday heating profile)",
    unit: None,
    description: "the 24-hour weekday heating setpoint profile, degrees Fahrenheit",
};

/// the 24-hour weekend schedule fractions (extension)
pub static WEEKEND_SCHEDULE_FRACTIONS: HpxmlElement = HpxmlElement {
    name: "WeekendScheduleFractions",
    context: "the 24-hour weekend schedule fractions (extension; a location prefix may name it)",
    unit: None,
    description: "the 24-hour weekend schedule fractions (extension)",
};

/// the 24-hour weekend cooling setpoint profile, degrees Fahrenheit
pub static WEEKEND_SETPOINT_TEMPS_COOLING_SEASON: HpxmlElement = HpxmlElement {
    name: "WeekendSetpointTempsCoolingSeason",
    context: "HVACControl/extension (the 24-hour weekend cooling profile)",
    unit: None,
    description: "the 24-hour weekend cooling setpoint profile, degrees Fahrenheit",
};

/// the 24-hour weekend heating setpoint profile, degrees Fahrenheit
pub static WEEKEND_SETPOINT_TEMPS_HEATING_SEASON: HpxmlElement = HpxmlElement {
    name: "WeekendSetpointTempsHeatingSeason",
    context: "HVACControl/extension (the 24-hour weekend heating profile)",
    unit: None,
    description: "the 24-hour weekend heating setpoint profile, degrees Fahrenheit",
};

/// the <Window> element the parser reads under its parent
pub static WINDOW: HpxmlElement = HpxmlElement {
    name: "Window",
    context: "BuildingDetails/Enclosure/Windows/Window",
    unit: None,
    description: "the <Window> element the parser reads under its parent",
};

/// the <Windows> element the parser reads under its parent
pub static WINDOWS: HpxmlElement = HpxmlElement {
    name: "Windows",
    context: "BuildingDetails/Enclosure/Windows | BuildingDetails/Systems/HVAC/HVACPlant/extension/DesignLoads/CoolingSensible/Windows | BuildingDetails/Systems/HVAC/HVACPlant/extension/DesignLoads/Heating/Windows",
    unit: None,
    description: "the <Windows> element the parser reads under its parent",
};

/// the <WinterShadingCoefficient> element the parser reads under its parent
pub static WINTER_SHADING_COEFFICIENT: HpxmlElement = HpxmlElement {
    name: "WinterShadingCoefficient",
    context: "BuildingDetails/Enclosure/Windows/Window/ExteriorShading/WinterShadingCoefficient | BuildingDetails/Enclosure/Windows/Window/InteriorShading/WinterShadingCoefficient",
    unit: None,
    description: "the <WinterShadingCoefficient> element the parser reads under its parent",
};

/// the <WithinInfiltrationVolume> element the parser reads under its parent
pub static WITHIN_INFILTRATION_VOLUME: HpxmlElement = HpxmlElement {
    name: "WithinInfiltrationVolume",
    context: "BuildingDetails/Enclosure/Attics/Attic/WithinInfiltrationVolume",
    unit: None,
    description: "the <WithinInfiltrationVolume> element the parser reads under its parent",
};

/// Every element the parser reads. The accessors take these by
/// reference, so the list is the parser's own vocabulary, never a
/// parallel list.
pub static HPXML_ELEMENTS: &[&HpxmlElement] = &[
    &ABOVE_APARTMENT,
    &ACTIVE_DURATION_S,
    &ACTIVE_POWER_W,
    &ADJUSTED_ANNUALK_WH,
    &ADVANCED_RESEARCH_FEATURES,
    &AEXT,
    &AFUE,
    &AIR_DISTRIBUTION,
    &AIRFLOW,
    &AIRFLOW_DEFECT_RATIO,
    &AIR_INFILTRATION_MEASUREMENT,
    &AIR_LEAKAGE,
    &ALTITUDE,
    &AMBIENT,
    &ANNUAL_CONSUMPTIONK_BTU,
    &ANNUAL_COOLING_EFFICIENCY,
    &ANNUAL_HEATING_EFFICIENCY,
    &ANNUAL_OUTPUTK_WH,
    &APPLIANCES,
    &AREA,
    &AREA_FRACTION,
    &ARRAY_AZIMUTH,
    &ARRAY_ORIENTATION,
    &ARRAY_TILT,
    &ASSEMBLY_EFFECTIVE_R_VALUE,
    &ATTACHED_TO_FLOOR,
    &ATTACHED_TO_HOT_WATER_DISTRIBUTION,
    &ATTACHED_TO_INVERTER,
    &ATTACHED_TO_POOL,
    &ATTACHED_TO_ROOF,
    &ATTACHED_TO_WALL,
    &ATTACHED_TO_WATER_HEATING_SYSTEM,
    &ATTIC,
    &ATTICS,
    &ATTIC_TYPE,
    &AUTOSIZING_LIMITS,
    &AVERAGE_CEILING_HEIGHT,
    &AZIMUTH,
    &BACKUP_ANNUAL_HEATING_EFFICIENCY,
    &BACKUP_HEATING_AUTOSIZING_FACTOR,
    &BACKUP_HEATING_CAPACITY,
    &BACKUP_HEATING_LOCKOUT_TEMPERATURE,
    &BACKUP_HEATING_SWITCHOVER_TEMPERATURE,
    &BACKUP_SYSTEM_FUEL,
    &BACKUP_TYPE,
    &BASEMENT,
    &BATTERIES,
    &BATTERY,
    &BATTERY_CAPACITY,
    &BEGIN_DAY_OF_MONTH,
    &BEGIN_HOUR_OF_DAY,
    &BEGIN_MONTH,
    &BRANCH_PIPING_LOOP_LENGTH,
    &BUILDING,
    &BUILDING_AIR_LEAKAGE,
    &BUILDING_CONSTRUCTION,
    &BUILDING_DETAILS,
    &BUILDING_OCCUPANCY,
    &BUILDING_SUMMARY,
    &CAPACITY,
    &CEILING_FAN,
    &CHARGE_DEFECT_RATIO,
    &CHARGING_LEVEL,
    &CLIMATEAND_RISK_ZONES,
    &CLIMATE_ZONE,
    &CLIMATE_ZONE_IECC,
    &CLOTHES_DRYER,
    &CLOTHES_WASHER,
    &COLUMN_NAME,
    &COMBINED_ENERGY_FACTOR,
    &COMPRESSOR_LOCKOUT_TEMPERATURE,
    &COMPRESSOR_TYPE,
    &CONDITIONED,
    &CONDITIONED_BUILDING_VOLUME,
    &CONDITIONED_FLOOR_AREA,
    &CONDUCTIVITY,
    &COOKING_RANGE,
    &COOLDOWN_DURATION_S,
    &COOLING_AIRFLOW_CFM,
    &COOLING_AUTOSIZING_FACTOR,
    &COOLING_CAPACITY,
    &COOLING_SENSIBLE_HEAT_FRACTION,
    &COOLING_SYSTEM,
    &COOLING_SYSTEM_FUEL,
    &COOLING_SYSTEM_TYPE,
    &COP,
    &COUNT,
    &CRANKCASE_HEATER_POWER_WATTS,
    &CRANKCASE_HEATER_WATTS,
    &CRAWLSPACE,
    &DEHUMIDIFIER,
    &DEHUMIDISTAT_SETPOINT,
    &DENSITY,
    &DEPTH_BELOW_GRADE,
    &DISHWASHER,
    &DISTANCE_TO_BOTTOM_OF_INSULATION,
    &DISTANCE_TO_TOP_OF_INSULATION,
    &DOOR,
    &DOORS,
    &DUCT_INSULATION_R_VALUE,
    &DUCT_LEAKAGE,
    &DUCT_LEAKAGE_MEASUREMENT,
    &DUCT_LOCATION,
    &DUCTS,
    &DUCT_SURFACE_AREA,
    &DUCT_TYPE,
    &EER,
    &EER_2,
    &EFFECTIVE_LEAKAGE_AREA,
    &EFFICIENCY,
    &ELECTRICAL_POWER_OUTPUT,
    &ELECTRIC_AUXILIARY_ENERGY,
    &ELECTRIC_VEHICLE,
    &ELECTRIC_VEHICLES,
    &ELEVATION,
    &EMITTANCE,
    &ENCLOSURE,
    &END_DAY_OF_MONTH,
    &END_HOUR_OF_DAY,
    &END_MONTH,
    &ENERGY_FACTOR,
    &ERI_CALCULATION,
    &EVENT_PROBABILITY_CONSTANT,
    &EVENT_PROBABILITY_SCHEDULE_COL,
    &EVENT_PROBABILITY_SOURCE,
    &EVENT_WINDOW_SCHEDULE_COL,
    &EVENT_WINDOW_SOURCE,
    &EXPOSED_PERIMETER,
    &EXTENSION,
    &EXTERIOR_ADJACENT_TO,
    &EXTERIOR_SHADING,
    &FAN_POWER,
    &FAN_POWER_WATTS,
    &FAN_POWER_WATTS_PER_CFM,
    &FAN_TYPE,
    &FIRST_HOUR_RATING,
    &FLAT_ROOF,
    &FLOOR,
    &FLOOR_AREA,
    &FLOOR_OR_CEILING,
    &FLOORS,
    &FLOOR_TYPE,
    &FLUE_LOSS_FRACTION,
    &FOUNDATION,
    &FOUNDATIONS,
    &FOUNDATION_TYPE,
    &FOUNDATION_WALL,
    &FOUNDATION_WALLS,
    &FRAC_LATENT,
    &FRAC_RADIANT,
    &FRAC_SENSIBLE,
    &FRACTION_COOLING_LOAD_SERVED,
    &FRACTION_COOL_LOAD_SERVED,
    &FRACTION_DEHUMIDIFICATION_LOAD_SERVED,
    &FRACTION_DHW_LOAD_SERVED,
    &FRACTION_HEATING_LOAD_SERVED,
    &FRACTION_HEAT_LOAD_SERVED,
    &FRACTION_LOAD_SERVED,
    &FRACTIONOF_UNITS_IN_LOCATION,
    &FRACTION_OPERABLE,
    &FRAME_FLOOR,
    &FRAME_FLOORS,
    &FRAMING_FACTOR,
    &FREEZER,
    &FUEL_LOAD,
    &FUEL_LOAD_TYPE,
    &FUEL_TYPE,
    &FURNITURE_MASS,
    &GARAGE,
    &GARAGES,
    &GENERATOR,
    &GENERATORS,
    &HAS_FLUE_OR_CHIMNEY_IN_CONDITIONED_SPACE,
    &HEATER,
    &HEATING_AIRFLOW_CFM,
    &HEATING_AUTOSIZING_FACTOR,
    &HEATING_CAPACITY,
    &HEATING_CAPACITY_17_F,
    &HEATING_SYSTEM,
    &HEATING_SYSTEM_FUEL,
    &HEATING_SYSTEM_TYPE,
    &HEAT_PUMP,
    &HEAT_PUMP_TYPE,
    &HEIGHT,
    &HOT_TUB,
    &HOT_TUB_PUMP,
    &HOT_TUB_PUMPS,
    &HOT_TUBS,
    &HOT_WATER_DISTRIBUTION,
    &HOT_WATER_TEMPERATURE,
    &HOURS_IN_OPERATION,
    &HPXML,
    &HSPF,
    &HSPF_2,
    &HVAC,
    &HVAC_CONTROL,
    &HVAC_PLANT,
    &IGNITION_TYPE,
    &INFILTRATION_HEIGHT,
    &INSULATION,
    &INSULATION_DEPTH,
    &INSULATION_R_VALUE,
    &INSULATION_SPANS_ENTIRE_SLAB,
    &INSULATION_WIDTH,
    &INTEGRATED_ENERGY_FACTOR,
    &INTEGRATED_MODIFIED_ENERGY_FACTOR,
    &INTERIOR_ADJACENT_TO,
    &INTERIOR_SHADING,
    &INVERTER,
    &INVERTER_EFFICIENCY,
    &IS_INDUCTION,
    &IS_SHARED_APPLIANCE,
    &JACKET,
    &JACKET_R_VALUE,
    &LABEL_ANNUAL_GAS_COST,
    &LABEL_ELECTRIC_RATE,
    &LABEL_GAS_RATE,
    &LABEL_USAGE,
    &LATENT_GAIN_FRACTION,
    &LATITUDE,
    &LAYER,
    &LENGTH,
    &LIGHTING,
    &LIGHTING_GROUP,
    &LIGHTING_TYPE,
    &LOAD,
    &LOCATION,
    &LONGITUDE,
    &LOW_FLOW,
    &MAX_CAPACITY,
    &MAX_CHARGING_POWER,
    &MAX_POWER_OUTPUT,
    &MECHANICAL_VENTILATION,
    &MICROWAVE,
    &MIN_CAPACITY,
    &MINIMUM_CAPACITY,
    &MISC_LOADS,
    &MODULE_TYPE,
    &MONTHLY_SCHEDULE_MULTIPLIERS,
    &NAME,
    &NATURAL_VENTILATION,
    &NOMINAL_CAPACITY,
    &NOMINAL_R_VALUE,
    &NUMBEROF_BEDROOMS,
    &NUMBEROF_CONDITIONED_FLOORS,
    &NUMBEROF_CONDITIONED_FLOORS_ABOVE_GRADE,
    &NUMBEROF_RESIDENTS,
    &PERFORMANCE_ADJUSTMENT,
    &PERIMETER,
    &PERIMETER_INSULATION,
    &PERMANENT_SPA,
    &PHOTOVOLTAICS,
    &PILOT_POWER,
    &PIPE_INSULATION,
    &PIPE_R_VALUE,
    &PIPING_LENGTH,
    &PITCH,
    &PLACE_SETTING_CAPACITY,
    &PLUG_LOAD,
    &PLUG_LOAD_TYPE,
    &POOL,
    &POOL_PUMP,
    &POOL_PUMPS,
    &POOLS,
    &PORTABLE_ELECTRIC_SPA,
    &PRIMARY_COOLING_SYSTEM,
    &PRIMARY_HEATING_SYSTEM,
    &PRIMARY_INDICATOR,
    &PRIMARY_SYSTEMS,
    &PV_SYSTEM,
    &RADIANT_BARRIER,
    &RATED_ANNUALK_WH,
    &RATED_FLOW_RATE,
    &RATED_POWER_OUTPUT,
    &RECIRCULATION,
    &RECOVERY_EFFICIENCY,
    &REFRIGERATOR,
    &RELATED_HVAC_SYSTEM,
    &RESIDENTIAL_FACILITY_TYPE,
    &RIM_JOIST,
    &RIM_JOISTS,
    &ROOF,
    &ROOFS,
    &ROOF_TYPE,
    &ROUND_TRIP_EFFICIENCY,
    &R_VALUE,
    &SEER,
    &SEER_2,
    &SENSIBLE_GAIN_FRACTION,
    &SENSIBLE_HEAT_FRACTION,
    &SENSIBLE_RECOVERY_EFFICIENCY,
    &SETBACK_START_HOUR_HEATING,
    &SETBACK_TEMP_HEATING_SEASON,
    &SETPOINT_TEMP_COOLING_SEASON,
    &SETPOINT_TEMP_HEATING_SEASON,
    &SETUP_START_HOUR_COOLING,
    &SETUP_TEMP_COOLING_SEASON,
    &SHGC,
    &SHIELDINGOF_HOME,
    &SIDING,
    &SIMULATION_CONTROL,
    &SITE,
    &SITE_TYPE,
    &SKYLIGHT,
    &SKYLIGHTS,
    &SLAB,
    &SLABS,
    &SLAB_ON_GRADE,
    &SOFTWARE_INFO,
    &SOLAR_ABSORPTANCE,
    &SPAS,
    &SPECIFIC_HEAT,
    &STANDARD,
    &STANDBY_POWER,
    &STUD_SPACING,
    &STUD_WIDTH,
    &SUMMER_SHADING_COEFFICIENT,
    &SUPPLEMENTAL_HEATING_LOCKOUT_TEMPERATURE,
    &SYSTEM_IDENTIFIER,
    &SYSTEM_LOSSES_FRACTION,
    &SYSTEMS,
    &SYSTEM_TYPE,
    &TANK_HEIGHT,
    &TANK_VOLUME,
    &TEMPERATURE,
    &TEMPERATURE_CAPACITANCE_MULTIPLIER,
    &TEXT,
    &THICKNESS,
    &TILT,
    &TIME_ZONE,
    &TOTAL_ANNUAL_POWER_CONSUMPTION_IN_STANDBY_MODE,
    &TOTAL_RECOVERY_EFFICIENCY,
    &TOTAL_SETBACK_HOURSPER_WEEK_HEATING,
    &TOTAL_SETUP_HOURSPER_WEEK_COOLING,
    &TRACKING,
    &TYPE,
    &TYPE_OF_INFILTRATION_LEAKAGE,
    &U_FACTOR,
    &UNAVAILABLE_PERIOD,
    &UNAVAILABLE_PERIODS,
    &UNDER_SLAB_INSULATION,
    &UNIFORM_ENERGY_FACTOR,
    &UNIT_HEIGHT_ABOVE_GRADE,
    &UNITOF_MEASURE,
    &UNITS,
    &USAGE,
    &USAGE_MULTIPLIER,
    &USED_FOR_SEASONAL_COOLING_LOAD_REDUCTION,
    &USED_FOR_WHOLE_BUILDING_VENTILATION,
    &UTC_OFFSET,
    &VALUE,
    &VENTED,
    &VENTILATION_FAN,
    &VENTILATION_FANS,
    &VENTILATION_RATE,
    &VERSION,
    &WALL,
    &WALL_HEIGHT,
    &WALLS,
    &WALL_TYPE,
    &WATER_FIXTURE,
    &WATER_FIXTURES_USAGE_MULTIPLIER,
    &WATER_HEATER_AUTOSIZING_FACTOR,
    &WATER_HEATER_INSULATION,
    &WATER_HEATER_TYPE,
    &WATER_HEATING,
    &WATER_HEATING_SYSTEM,
    &WEATHER_STATION,
    &WEEKDAY_SCHEDULE_FRACTIONS,
    &WEEKDAY_SETPOINT_TEMPS_COOLING_SEASON,
    &WEEKDAY_SETPOINT_TEMPS_HEATING_SEASON,
    &WEEKEND_SCHEDULE_FRACTIONS,
    &WEEKEND_SETPOINT_TEMPS_COOLING_SEASON,
    &WEEKEND_SETPOINT_TEMPS_HEATING_SEASON,
    &WINDOW,
    &WINDOWS,
    &WINTER_SHADING_COEFFICIENT,
    &WITHIN_INFILTRATION_VOLUME,
];

/// Every element whose presence fails the parse. The parser's
/// rejection scan runs over this list, so a caller can see what the
/// parser refuses and why.
pub static HPXML_REJECTED_ELEMENTS: &[&RejectedHpxmlElement] =
    &[&HAS_FLUE_OR_CHIMNEY_REJECTED, &ACTIVE_POWER_KW_REJECTED];

/// The legacy flue flag: HARES reads `HasFlueOrChimneyInConditionedSpace`,
/// never the older name; the parse's rejection scan searches the whole
/// tree's names for it (the fixture form is under the air-infiltration
/// measurement's extension).
/// The old synthetic extension element: HARES reads `active_power_w`
/// (SI); the kW spelling is refused, naming the replacement.
pub static ACTIVE_POWER_KW_REJECTED: RejectedHpxmlElement = RejectedHpxmlElement {
    name: "active_power_kw",
    context: "BuildingDetails/Appliances/CookingRange/extension",
    reason: "removed element <active_power_kw>; rename it to <active_power_w> (W)",
};

pub static HAS_FLUE_OR_CHIMNEY_REJECTED: RejectedHpxmlElement = RejectedHpxmlElement {
    name: "HasFlueOrChimney",
    context: "AirInfiltrationMeasurement/extension",
    reason: "deprecated element <HasFlueOrChimney>; rename it to
             <HasFlueOrChimneyInConditionedSpace>",
};

/// The FoundationType choice children the parser's dispatches name:
/// the foundation kind's arms and the conditioned and vented reads
/// under them. The dispatch matches the child against these by
/// identity, so a choice arm must be one of the declared statics.
pub static FOUNDATION_TYPE_CHOICES: &[&HpxmlElement] = &[
    &CRAWLSPACE,
    &BASEMENT,
    &SLAB_ON_GRADE,
    &AMBIENT,
    &ABOVE_APARTMENT,
];

/// The type-element children the infiltration reader's space-location
/// match names, for either type element it reads (an AtticType or a
/// FoundationType).
pub static SPACE_LOCATION_CHOICES: &[&HpxmlElement] = &[&ATTIC, &BASEMENT, &CRAWLSPACE];

/// The AtticType choice children the parser's reads name: the attic
/// cavity kind the zone builder skips (a flat roof means no attic
/// cavity) and the space-location match's attic kind.
pub static ATTIC_TYPE_CHOICES: &[&HpxmlElement] = &[&ATTIC, &FLAT_ROOF];

/// Every declared element's name, for the coverage test's membership
/// check.
#[cfg(test)]
pub(crate) fn declared_names() -> std::collections::HashSet<&'static str> {
    HPXML_ELEMENTS.iter().map(|element| element.name).collect()
}

/// The rejected elements' names, for the coverage test's filter.
#[cfg(test)]
pub(crate) fn rejected_names() -> std::collections::HashSet<&'static str> {
    HPXML_REJECTED_ELEMENTS
        .iter()
        .map(|entry| entry.name)
        .collect()
}

/// The reads the accessors recorded on this thread since the last take.
/// The coverage test parses every fixture and reads the record back.
#[cfg(test)]
pub(crate) mod read_recorder {
    use std::cell::RefCell;

    thread_local! {
        static READS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn record(name: &'static str) {
        READS.with(|reads| reads.borrow_mut().push(name));
    }

    #[must_use]
    pub(crate) fn take() -> Vec<&'static str> {
        READS.with(|reads| std::mem::take(&mut *reads.borrow_mut()))
    }
}
