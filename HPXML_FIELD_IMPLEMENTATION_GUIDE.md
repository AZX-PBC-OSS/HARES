# HPXML Field Implementation Guide: OCHRE to HARES

This document traces each HPXML field from parsing through simulation usage, comparing OCHRE and HARES implementations and providing specific implementation proposals.

---

## Table of Contents
1. [Building Summary Fields](#1-building-summary-fields)
2. [Enclosure Fields](#2-enclosure-fields)
3. [HVAC Fields](#3-hvac-fields)
4. [Water Heating Fields](#4-water-heating-fields)
5. [DER Fields (PV, Battery, Generator)](#5-der-fields)
6. [Appliance Fields](#6-appliance-fields)
7. [Lighting Fields](#7-lighting-fields)
8. [Miscellaneous Load Fields](#8-miscellaneous-load-fields)
9. [Implementation Priority Matrix](#9-implementation-priority-matrix)

---

## 1. Building Summary Fields

### 1.1 `ResidentialFacilityType`

**HPXML Path**: `BuildingSummary/BuildingConstruction/ResidentialFacilityType`

**OCHRE Implementation**:
- **Parsed**: `construction["ResidentialFacilityType"]` in `parse_hpxml_boundaries()`
- **Used**: Stored in `construction_dict["House Type"]`
- **Simulation Impact**: Affects default envelope properties, infiltration assumptions
  - Mobile homes: Different thermal characteristics
  - Single-family vs multi-family: Different zone configurations
  - Apartment vs house: Different envelope assumptions

**Current HARES Status**: ❌ NOT IMPLEMENTED

**Proposed HARES Implementation**:
```rust
// In building.rs - add to Building struct
pub struct Building {
    pub house_type: Option<HouseType>,
    // ... existing fields
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HouseType {
    SingleFamilyDetached,
    SingleFamilyAttached,
    Apartment,  // for multifamily
    MobileHome,
    Condo,
    Townhouse,
    Other(String),
}
```

**Simulation Usage**:
- Input to envelope defaults selection
- Affects infiltration calculations (mobile homes leakier)
- May affect internal mass assumptions

**Implementation Priority**: 🔴 **HIGH** - Required for accurate mobile home modeling

---

### 1.2 `NumberofConditionedFloors` vs `NumberofConditionedFloorsAboveGrade`

**HPXML Paths**: 
- `BuildingSummary/BuildingConstruction/NumberofConditionedFloors`
- `BuildingSummary/BuildingConstruction/NumberofConditionedFloorsAboveGrade`

**OCHRE Implementation**:
```python
total_floors = construction["NumberofConditionedFloors"]
indoor_floors = construction.get("NumberofConditionedFloorsAboveGrade", total_floors)
# Used to determine foundation type:
# if total_floors > indoor_floors: has basement/finished basement
```

**Current HARES Status**: ⚠️ **PARTIAL** - Only `NumberofConditionedFloorsAboveGrade` parsed

**Proposed HARES Implementation**:
```rust
// Add to Building struct
pub total_floors: Option<f64>,
pub floors_above_grade: Option<f64>,  // rename from existing floors_above_grade

// Logic for foundation detection:
if let (Some(total), Some(above)) = (total_floors, floors_above_grade) {
    if total > above {
        // Has basement (conditioned or not)
        foundation_zone_exists = true;
    }
}
```

**Simulation Usage**:
- Foundation zone existence detection
- Infiltration height calculations
- Lighting area calculations (finished basement)

**Implementation Priority**: 🟡 **MEDIUM** - Needed for proper basement handling

---

### 1.3 `NumberofBathrooms`

**HPXML Path**: `BuildingSummary/BuildingConstruction/NumberofBathrooms`

**OCHRE Implementation**:
```python
n_baths = construction.get("NumberofBathrooms", n_beds / 2 + 0.5)
# Used in water heater sizing calculations
```

**Current HARES Status**: ❌ NOT IMPLEMENTED

**Proposed HARES Implementation**:
```rust
pub number_of_bathrooms: Option<f64>,
// Default: bedrooms / 2.0 + 0.5 if not specified
```

**Simulation Usage**:
- Hot water draw calculations (minor impact - bedrooms dominates)
- Water heater sizing

**Implementation Priority**: 🟢 **LOW** - Has default formula, minor impact

---

### 1.4 `NumberofResidents` (Occupancy)

**HPXML Path**: `BuildingSummary/BuildingOccupancy/NumberofResidents`

**OCHRE Implementation**:
- Parsed into occupancy parameters
- Affects internal gains, moisture generation
- Used in schedule generation

**Current HARES Status**: ✅ **IMPLEMENTED** in `resolve_loads.rs`

**Simulation Usage**:
```rust
if let Some(n) = child_f64(occupancy, "NumberofResidents") {
    params.insert("number_of_occupants".to_string(), json!(n));
}
```

---

## 2. Enclosure Fields

### 2.1 `HasFlueOrChimneyInConditionedSpace`

**HPXML Path**: `Enclosure/AirInfiltration/extension/HasFlueOrChimneyInConditionedSpace`

**OCHRE Implementation**:
```python
has_flue = hpxml["Enclosure"]["AirInfiltration"]["extension"]["HasFlueOrChimneyInConditionedSpace"]
# Used in infiltration calculations (increases effective leakage)
```

**Current HARES Status**: ⚠️ **PARSED but NOT CONNECTED**
- Field exists in Building struct
- Not used in infiltration calculations

**Proposed HARES Implementation**:
```rust
// In infiltration calculations (when implemented):
let flue_multiplier = if building.has_flue_or_chimney.unwrap_or(false) {
    1.1  // 10% increase in infiltration
} else {
    1.0
};
let effective_ach50 = base_ach50 * flue_multiplier;
```

**Simulation Usage**:
- Modifies infiltration rate
- Accounts for stack effect from flues

**Implementation Priority**: 🟡 **MEDIUM** - Parsed but needs connection to physics

---

### 2.2 Attic `AttachedToRoof`, `AttachedToWall`, `AttachedToFloor`

**HPXML Paths**:
- `Enclosure/Attics/Attic/AttachedToRoof`
- `Enclosure/Attics/Attic/AttachedToWall`
- `Enclosure/Attics/Attic/AttachedToFloor`

**OCHRE Implementation**:
```python
# Resolves HPXML ID references to link attics to boundaries
for attic in attics:
    roof_id = attic["AttachedToRoof"]["@idref"]
    wall_ids = [w["@idref"] for w in attic.get("AttachedToWall", [])]
    floor_ids = [f["@idref"] for f in attic.get("AttachedToFloor", [])]
# Used to aggregate areas and properties
```

**Current HARES Status**: ❌ **NOT IMPLEMENTED** - ID references not resolved

**Proposed HARES Implementation**:
```rust
// Step 1: Parse all boundaries first, build ID map
struct HpxmlIdMap {
    roofs: HashMap<String, Roof>,
    walls: HashMap<String, Wall>,
    floors: HashMap<String, Floor>,
    attics: HashMap<String, Attic>,
    foundations: HashMap<String, Foundation>,
}

// Step 2: Resolve references after initial parse
impl HpxmlIdMap {
    fn resolve_attic_links(&mut self) {
        for (attic_id, attic) in &self.attics {
            if let Some(roof_id) = &attic.attached_to_roof_id {
                if let Some(roof) = self.roofs.get(roof_id) {
                    attic.roof = Some(roof.clone());
                }
            }
            // Similar for walls, floors
        }
    }
}
```

**Simulation Usage**:
- Aggregates boundary areas by zone
- Ensures consistent area calculations
- Links attic ventilation to roof properties

**Implementation Priority**: 🟡 **MEDIUM** - Complex to implement, affects area calculations

---

### 2.3 Foundation `AttachedToRimJoist`, `AttachedToFoundationWall`, `AttachedToSlab`

**HPXML Paths**:
- `Enclosure/Foundations/Foundation/AttachedToRimJoist`
- `Enclosure/Foundations/Foundation/AttachedToFoundationWall`
- `Enclosure/Foundations/Foundation/AttachedToSlab`

**OCHRE Implementation**: Same pattern as attic references - resolves ID links

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**: Same approach as attics - two-phase parsing with ID resolution

**Implementation Priority**: 🟡 **MEDIUM**

---

### 2.4 `FoundationType/Basement/Conditioned`

**HPXML Path**: `Enclosure/Foundations/Foundation/FoundationType/Basement/Conditioned`

**OCHRE Implementation**:
```python
foundation_type = foundation.get("FoundationType")
if isinstance(foundation_type, dict):
    foundation_name = list(foundation_type.keys())[0]  # Crawlspace or Basement
    if foundation_name == "Basement":
        if total_floors > indoor_floors:
            foundation_name = "Finished Basement"  # Conditioned
        else:
            foundation_name = "Unfinished Basement"  # Unconditioned
```

**Current HARES Status**: ❌ **NOT IMPLEMENTED** - Only basic FoundationType enum

**Proposed HARES Implementation**:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoundationType {
    SlabOnGrade,
    Crawlspace { vented: bool },
    Basement { conditioned: bool },  // Add conditioned field
    Ambient,
    AboveApartment,
    Other(String),
}

// Parse logic:
if let Some(basement) = foundation_type_node.child("Basement") {
    let conditioned = basement.child("Conditioned")
        .map(|n| n.text.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    FoundationType::Basement { conditioned }
}
```

**Simulation Usage**:
- Determines if foundation zone is conditioned or not
- Affects HVAC load calculations
- Changes infiltration assumptions

**Implementation Priority**: 🔴 **HIGH** - Affects zone thermal coupling

---

### 2.5 Window `WinterShadingCoefficient`

**HPXML Path**: `Enclosure/Windows/Window/InteriorShading/WinterShadingCoefficient`

**OCHRE Implementation**:
```python
# Only summer coefficient is used:
shading = bd_data.get("InteriorShading", {}).get("SummerShadingCoefficient")
# Winter coefficient parsed but not utilized in simulation
```

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
pub struct Window {
    // ... existing fields ...
    pub interior_shading_fraction_summer: f64,
    pub interior_shading_fraction_winter: f64,
}

// In simulation, apply seasonal shading:
fn get_effective_shgc(&self, month: u32) -> f64 {
    let is_winter = month >= 11 || month <= 3;
    let shading = if is_winter {
        self.interior_shading_fraction_winter
    } else {
        self.interior_shading_fraction_summer
    };
    self.shgc.unwrap_or(0.6) * shading
}
```

**Simulation Usage**:
- Seasonal variation in solar heat gains
- Higher winter shading = more solar heating
- Lower summer shading = less cooling load

**Implementation Priority**: 🟡 **MEDIUM** - 5-15% seasonal impact on solar gains

---

### 2.6 `FractionOperable` (Windows)

**HPXML Path**: `Enclosure/Windows/Window/FractionOperable`

**OCHRE Implementation**: Parsed but not used in current simulation

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
pub operable_fraction: Option<f64>,

// Could be used for natural ventilation modeling (future feature)
```

**Simulation Usage**: Not currently used by either tool (natural ventilation not modeled)

**Implementation Priority**: 🟢 **LOW** - Not used in current physics

---

### 2.7 Internal Mass - `PartitionWallMass` and `FurnitureMass`

**HPXML Paths**:
- `Enclosure/extension/PartitionWallMass/AreaFraction`
- `Enclosure/extension/PartitionWallMass/InteriorFinish/Type`
- `Enclosure/extension/PartitionWallMass/InteriorFinish/Thickness`
- `Enclosure/extension/FurnitureMass/AreaFraction`

**OCHRE Implementation**:
```python
# Calculated from floor area, not HPXML:
ZONE_FURNITURE_AREA_FRACTIONS = {
    "Indoor": 0.4,
    "Foundation": 0.4,
    "Garage": 0.1,
    "Attic": 0,
}
# Partition wall mass is a fixed assumption
```

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
pub struct InternalMass {
    pub partition_wall_area_fraction: f64,
    pub partition_wall_finish_type: Option<String>,
    pub partition_wall_finish_thickness_m: Option<f64>,
    pub furniture_area_fraction: f64,
}

// Default if not in HPXML:
impl Default for InternalMass {
    fn default() -> Self {
        Self {
            partition_wall_area_fraction: 1.0,  # 1x floor area
            partition_wall_finish_type: None,
            partition_wall_finish_thickness_m: None,
            furniture_area_fraction: 0.4,  # OCHRE default for conditioned
        }
    }
}
```

**Simulation Usage**:
- Adds thermal capacitance to zones
- Reduces temperature swing amplitude
- Delays response to load changes

**Implementation Priority**: 🟡 **MEDIUM** - Affects thermal dynamics

---

### 2.8 Slab `CarpetFraction` and `CarpetRValue`

**HPXML Paths**:
- `Enclosure/Slabs/Slab/extension/CarpetFraction`
- `Enclosure/Slabs/Slab/extension/CarpetRValue`

**OCHRE Implementation**: Not fully implemented in current version

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
pub struct Slab {
    // ... existing fields ...
    pub carpet_fraction: Option<f64>,  # 0.0 to 1.0
    pub carpet_r_value_m2_k_w: Option<f64>,
}

// In thermal calculation:
let effective_r = match (slab.carpet_fraction, slab.carpet_r_value_m2_k_w) {
    (Some(frac), Some(carpet_r)) => {
        let floor_r = slab.r_value;
        1.0 / ((1.0 - frac) / floor_r + frac / (floor_r + carpet_r))
    }
    _ => slab.r_value,
};
```

**Simulation Usage**:
- Reduces slab heat loss in carpeted areas
- Affects floor surface temperature

**Implementation Priority**: 🟢 **LOW** - Minor impact (10% of floor area typically)

---

### 2.9 Foundation Wall Details

**HPXML Fields**:
- `Thickness`
- `DepthBelowGrade`
- `InteriorFinish/Type`
- `Insulation/Layer/DistanceToTopOfInsulation`
- `Insulation/Layer/DistanceToBottomOfInsulation`

**OCHRE Implementation**: Uses simplified UA calculations from effective R-value

**Current HARES Status**: ⚠️ **PARTIAL** - Height, Area parsed; details missing

**Proposed HARES Implementation**:
```rust
pub struct FoundationWall {
    pub thickness_m: Option<f64>,
    pub depth_below_grade_m: Option<f64>,
    pub interior_finish_type: Option<String>,
    pub insulation_top_m: Option<f64>,  # Distance from top
    pub insulation_bottom_m: Option<f64>,  # Distance from top
}

// For simplified modeling, use effective R-value
// For detailed modeling, could calculate R-value by height segment
```

**Simulation Usage**:
- Detailed: Could model partial insulation, ground coupling
- Current: Effective R-value is sufficient

**Implementation Priority**: 🟢 **LOW** - Effective R-value dominates

---

## 3. HVAC Fields

### 3.1 `AnnualDistributionSystemEfficiency`

**HPXML Paths**:
- `Systems/HVAC/HVACDistribution/AnnualHeatingDistributionSystemEfficiency`
- `Systems/HVAC/HVACDistribution/AnnualCoolingDistributionSystemEfficiency`

**OCHRE Implementation**:
```python
# Alternative to detailed duct calculation:
# If DSE provided and no duct details, use DSE directly
# Otherwise, calculate DSE from ASHRAE 152
```

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
// In resolve_hvac.rs:
if let Some(dse) = child_f64(hvac_dist, "AnnualHeatingDistributionSystemEfficiency") {
    params.insert("heating_dse".to_string(), json!(dse));
}

// In HVAC core - use DSE if no duct parameters:
if duct_params.is_empty() && dse.is_some() {
    return dse.unwrap();  # Use HPXML DSE directly
} else {
    calculate_dse_from_ashrae152(duct_params)?
}
```

**Simulation Usage**:
- Simple alternative to full duct modeling
- Many HPXML files use this instead of detailed ducts
- Directly affects delivered capacity

**Implementation Priority**: 🔴 **HIGH** - Required for files without detailed ducts

---

### 3.2 `HeatingAirflowCFM` and `CoolingAirflowCFM`

**HPXML Paths**:
- `Systems/HVAC/HVACDistribution/extension/HeatingAirflowCFM`
- `Systems/HVAC/HVACDistribution/extension/CoolingAirflowCFM`

**OCHRE Implementation**: Used in ideal capacity algorithm and duct calculations

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
if let Some(af) = child_f64(ext, "HeatingAirflowCFM") {
    params.insert("heating_airflow_m3_s".to_string(), json!(conv::flow_cfm_to_m3_s(af)));
}
```

**Simulation Usage**:
- Airflow for sensible heat calculations
- Duct heat transfer calculations

**Implementation Priority**: 🟡 **MEDIUM** - Used in detailed HVAC models

---

### 3.3 Ventilation `HoursInOperation`

**HPXML Path**: `Systems/MechanicalVentilation/VentilationFans/VentilationFan/HoursInOperation`

**OCHRE Implementation**:
```python
assert vent_fan.get("HoursInOperation", 24) == 24  # Requires 24 hours
```

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
// Currently assumes 24/7 operation
// If HoursInOperation != 24, could scale ventilation rate:
let hours = child_f64(fan, "HoursInOperation").unwrap_or(24.0);
if hours != 24.0 {
    // Scale to achieve same daily volume in operating hours
    let scale_factor = 24.0 / hours;
    params.insert("ventilation_rate_cfm".to_string(), json!(flow_cfm * scale_factor));
    // Or implement intermittent schedule
}
```

**Simulation Usage**: Intermittent ventilation (e.g., 8 hrs/day)

**Implementation Priority**: 🟢 **LOW** - Most systems run 24/7

---

## 4. Water Heating Fields

### 4.1 `FractionDHWLoadServed`

**HPXML Path**: `Systems/WaterHeating/WaterHeatingSystem/FractionDHWLoadServed`

**OCHRE Implementation**: Used when multiple water heaters exist

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
if let Some(frac) = child_f64(wh, "FractionDHWLoadServed") {
    params.insert("fraction_load_served".to_string(), json!(frac));
}

// In draw profile calculation:
let wh_draw_l = total_draw_l * fraction_load_served;
```

**Simulation Usage**: Multiple water heaters sharing load

**Implementation Priority**: 🟡 **MEDIUM** - Multiple water heater scenarios

---

### 4.2 `TankHeight`

**HPXML Path**: `Systems/WaterHeating/WaterHeatingSystem/TankHeight`

**OCHRE Implementation**: Used in stratification calculations

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
if let Some(height) = child_f64(wh, "TankHeight") {
    params.insert("tank_height_m".to_string(), json!(conv::length_ft_to_m(height)));
} else {
    // Default: derive from volume assuming cylindrical tank
    // h = V / (π * r²), assume r = 0.5 m for typical tank
}
```

**Simulation Usage**: Affects node height in stratified tank model

**Implementation Priority**: 🟢 **LOW** - Has reasonable defaults

---

### 4.3 Water Fixture Types and Schedules

**HPXML Paths**:
- `Systems/WaterHeating/WaterFixture/WaterFixtureType`
- `Systems/WaterHeating/extension/WaterFixturesWeekdayScheduleFractions`
- `Systems/WaterHeating/extension/WaterFixturesWeekendScheduleFractions`
- `Systems/WaterHeating/extension/WaterFixturesMonthlyScheduleMultipliers`

**OCHRE Implementation**: Basic aggregate draw profile only

**Current HARES Status**: ⚠️ **PARTIAL** - Aggregate draw only

**Proposed HARES Implementation**:
```rust
// Parse fixture types (shower, faucet, etc.)
// Each has different flow rates and schedules
// Aggregate or keep separate based on fidelity needs

pub enum WaterFixtureType {
    Shower,
    Faucet,
    Dishwasher,
    ClothesWasher,
    Bath,
    Other,
}

// For now: aggregate into single schedule
// Future: model fixture-specific schedules and flow rates
```

**Simulation Usage**: Fixture-specific flow rates and usage patterns

**Implementation Priority**: 🟢 **LOW** - Aggregate works for most cases

---

## 5. DER Fields

### 5.1 PV `SystemLossesFraction`

**HPXML Path**: `Systems/Photovoltaics/PVSystem/SystemLossesFraction`

**OCHRE Implementation**: Uses PVWatts default losses

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
pub system_losses_fraction: Option<f64>,  # 0.0 to 1.0

// In PV calculation:
let system_losses = self.system_losses_fraction.unwrap_or(0.14);  # 14% default
let dc_output = cell_temp_derated_power * (1.0 - system_losses);
```

**Simulation Usage**: Accounts for soiling, shading, wiring losses, age degradation

**Implementation Priority**: 🟡 **MEDIUM** - 5-20% impact on generation

---

### 5.2 PV `ModuleType`

**HPXML Path**: `Systems/Photovoltaics/PVSystem/ModuleType`

**OCHRE Implementation**: Used in PVWatts v8

**Current HARES Status**: ⚠️ **PARTIAL** - NOCT model only

**Proposed HARES Implementation**:
```rust
pub enum PvModuleType {
    Standard,    # 0.90 efficiency
    Premium,     # 0.95 efficiency
    ThinFilm,    # 0.80 efficiency
}

// Map to HARES PV model parameters:
impl PvModuleType {
    fn efficiency(&self) -> f64 {
        match self {
            Self::Standard => 0.90,
            Self::Premium => 0.95,
            Self::ThinFilm => 0.80,
        }
    }
}
```

**Simulation Usage**: Module efficiency affects NOCT calculation

**Implementation Priority**: 🟢 **LOW** - Current model works, this is refinement

---

### 5.3 Battery `UsableCapacity`

**HPXML Path**: `Systems/Batteries/Battery/UsableCapacity`

**OCHRE Implementation**: Uses nominal capacity only

**Current HARES Status**: ❌ **NOT IMPLEMENTED** - Uses nominal only

**Proposed HARES Implementation**:
```rust
pub struct Battery {
    pub nominal_capacity_kwh: f64,
    pub usable_capacity_kwh: Option<f64>,
    // ...
}

// Use usable for SOC limits:
let usable = self.usable_capacity_kwh.unwrap_or(self.nominal_capacity_kwh * 0.9);
let min_soc_energy = usable * min_soc;
let max_soc_energy = usable * max_soc;
```

**Simulation Usage**: SOC management should use usable, not nominal

**Implementation Priority**: 🔴 **HIGH** - Improves SOC accuracy

---

### 5.4 Battery `Location`

**HPXML Path**: `Systems/Batteries/Battery/Location`

**OCHRE Implementation**: Parsed but not used for thermal coupling

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
pub location: Option<String>,  # e.g., "conditioned space", "garage", "outside"

// Could affect thermal model for battery temperature
// Currently battery has internal thermal model independent of location
```

**Simulation Usage**: Thermal environment for battery (affects degradation)

**Implementation Priority**: 🟢 **LOW** - Has internal thermal model

---

## 6. Appliance Fields

### 6.1 `Location` (All Appliances)

**HPXML Paths**: `Appliances/*/Location`

**OCHRE Implementation**:
```python
# Used to determine heat gains to conditioned space:
if parse_zone_name(r.get("Location", default)) == "Indoor":
    annual_kwh_conditioned += r_energy
```

**Current HARES Status**: ❌ **NOT IMPLEMENTED** - All gains go to conditioned

**Proposed HARES Implementation**:
```rust
// Add location field to all appliances
pub location: Option<ZoneType>,

// In scheduled_load.rs - only add gains if location is conditioned:
if location == ZoneType::Conditioned {
    PortContribution::Thermal {
        zone: ZoneId::conditioned(),
        sensible_gain_w: power * frac_sensible,
        // ...
    }
}
```

**Simulation Usage**: Heat gains only to zone where appliance located

**Implementation Priority**: 🟡 **MEDIUM** - Affects zone load distribution

---

### 6.2 Appliance Label Rates/Costs

**HPXML Fields**:
- `LabelElectricRate`
- `LabelGasRate`
- `LabelAnnualGasCost`

**OCHRE Implementation**: Used to calculate energy use from rating data

**Current HARES Status**: ⚠️ **PARTIAL** - Some parsed, not all

**Proposed HARES Implementation**:
```rust
// These are only needed for rating calculations
// Not used in energy simulation (actual loads used)
// Keep for compatibility but mark as "for_rating_only"
```

**Simulation Usage**: Rating calculations only, not simulation

**Implementation Priority**: 🟢 **LOW** - Not used in simulation

---

## 7. Lighting Fields

### 7.1 `UsageMultiplier`

**HPXML Paths**: `Lighting/extension/*UsageMultiplier` (Interior, Exterior, Garage)

**OCHRE Implementation**:
```python
annual_kwh = base_kwh * extension.get(f"{location.capitalize()}UsageMultiplier", 1.0)
```

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
let usage_multiplier = ext
    .and_then(|n| child_f64(n, &format!("{}UsageMultiplier", capitalize(&location))))
    .unwrap_or(1.0);
let annual_kwh = base_kwh * usage_multiplier;
```

**Simulation Usage**: Simple scaling of lighting energy

**Implementation Priority**: 🟢 **LOW** - Calibration aid only

---

### 7.2 Lighting `Weekday/Weekend/Monthly` Schedules

**HPXML Paths**: `Lighting/extension/*WeekdayScheduleFractions`, etc.

**OCHRE Implementation**: Parsed but uses same schedule framework as other loads

**Current HARES Status**: ⚠️ **PARTIAL** - Month multipliers parsed for ceiling fan only

**Proposed HARES Implementation**:
```rust
// Use same schedule pattern as other loads
let weekday_fractions = ext
    .and_then(|n| parse_fractions(n, &format!("{}WeekdayScheduleFractions", location_cap)));
// ... similar for weekend, monthly
```

**Simulation Usage**: Lighting-specific usage patterns

**Implementation Priority**: 🟢 **LOW** - Uses default schedules

---

## 8. Miscellaneous Load Fields

### 8.1 Pool/Spa `Type`

**HPXML Paths**:
- `Pools/Pool/Type`
- `Pools/Pool/Pumps/Pump/Type`
- `Pools/Pool/Heater/Type`

**OCHRE Implementation**: Parsed but only Load value used

**Current HARES Status**: ❌ **NOT IMPLEMENTED**

**Proposed HARES Implementation**:
```rust
pub pool_type: Option<String>,  # e.g., "in ground", "above ground"
pub pump_type: Option<String>,   # e.g., "variable speed", "single speed"
pub heater_type: Option<String>,  # e.g., "electric resistance", "heat pump"

// Could affect default schedules and efficiencies
// Currently use aggregate loads only
```

**Simulation Usage**: Would affect default schedules and efficiencies

**Implementation Priority**: 🟢 **LOW** - Aggregate loads sufficient

---

## 9. Implementation Priority Matrix

| Field/Attribute | Current HARES | OCHRE Usage | Simulation Impact | Priority | Proposed Implementation |
|-------------------|---------------|-------------|-------------------|----------|------------------------|
| **Building Summary** |
| `ResidentialFacilityType` | ❌ | Defaults selection | High | 🔴 High | Add `HouseType` enum |
| `NumberofBathrooms` | ❌ | Water heater sizing | Low | 🟢 Low | Parse with default |
| **Enclosure** |
| `HasFlueOrChimney` | ⚠️ Parsed | Infiltration calc | Medium | 🟡 Medium | Connect to infiltration |
| Attic/Foundation `AttachedTo*` | ❌ | Area aggregation | Medium | 🟡 Medium | Two-phase ID resolution |
| Foundation `Conditioned` | ❌ | Zone thermal coupling | High | 🔴 High | Add to `FoundationType` enum |
| Window `WinterShading` | ❌ | Seasonal SHGC | Medium | 🟡 Medium | Seasonal effective SHGC |
| Window `FractionOperable` | ❌ | Natural ventilation | None | 🟢 Low | Skip for now |
| `PartitionWallMass` | ❌ | Thermal capacitance | Medium | 🟡 Medium | Add to `Building` |
| `FurnitureMass` | ❌ | Thermal capacitance | Medium | 🟡 Medium | Zone furniture fraction |
| Slab `CarpetFraction` | ❌ | Floor R-value | Low | 🟢 Low | Parallel resistance calc |
| Foundation Wall details | ⚠️ Partial | Ground coupling | Low | 🟢 Low | Effective R-value sufficient |
| **HVAC** |
| `AnnualDistributionSystemEfficiency` | ❌ | Duct efficiency | High | 🔴 High | Alternative to duct calc |
| `Heating/CoolingAirflowCFM` | ❌ | Sensible heat calc | Medium | 🟡 Medium | Add to HVAC params |
| Ventilation `HoursInOperation` | ❌ | Intermittent operation | Low | 🟢 Low | Scale to 24hr equivalent |
| **Water Heating** |
| `FractionDHWLoadServed` | ❌ | Multiple WHs | Medium | 🟡 Medium | Scale draw profile |
| `TankHeight` | ❌ | Stratification | Low | 🟢 Low | Default from volume |
| Fixture types/schedules | ⚠️ Partial | Draw profile | Low | 🟢 Low | Aggregate sufficient |
| **DER** |
| PV `SystemLossesFraction` | ❌ | Generation | Medium | 🟡 Medium | Apply to DC output |
| PV `ModuleType` | ❌ | Efficiency | Low | 🟢 Low | Map to efficiency |
| Battery `UsableCapacity` | ❌ | SOC management | High | 🔴 High | Use for SOC limits |
| Battery `Location` | ❌ | Thermal coupling | Low | 🟢 Low | Has internal thermal |
| **Appliances** |
| `Location` (all) | ❌ | Zone heat gains | Medium | 🟡 Medium | Add to all appliances |
| Label rates/costs | ⚠️ Partial | Rating calc | None | 🟢 Low | Mark as rating-only |
| **Lighting** |
| `UsageMultiplier` | ❌ | Energy scaling | Low | 🟢 Low | Simple multiplier |
| Lighting schedules | ⚠️ Partial | Usage pattern | Low | 🟢 Low | Use load schedule pattern |
| **Misc** |
| Pool/Spa types | ❌ | Defaults | Low | 🟢 Low | Aggregate loads sufficient |

---

## Implementation Strategy

### Phase 1: Critical Fixes (🔴 High Priority)
1. **Add `ResidentialFacilityType`** - Required for mobile home accuracy
2. **Implement Foundation `Conditioned` flag** - Affects zone thermal coupling
3. **Add DSE support** - Many files use this instead of detailed ducts
4. **Add Battery `UsableCapacity`** - Improves SOC accuracy vs nominal capacity

### Phase 2: Important Improvements (🟡 Medium Priority)
1. **Connect `HasFlueOrChimney` to infiltration** - Parsed but not used
2. **Implement ID reference resolution** - For attic/foundation links
3. **Add Window `WinterShadingCoefficient`** - Seasonal solar gain variation
4. **Add Internal Mass modeling** - Partition walls and furniture
5. **Add Appliance `Location` fields** - Proper zone heat gains
6. **Add HVAC airflow CFM values** - For detailed sensible heat

### Phase 3: Nice to Have (🟢 Low Priority)
1. **Slab carpet properties** - Minor floor heat transfer impact
2. **Lighting multipliers and schedules** - Calibration aids
3. **Pool/Spa types** - Default schedules sufficient
4. **Label rates/costs** - Not used in simulation physics
5. **Ventilation hours** - Most systems run 24/7
6. **Detailed foundation wall insulation** - Effective R-value works

### Phase 4: Future Enhancements
1. **Natural ventilation** - Would use `FractionOperable`
2. **Fixture-specific water draws** - Would use `WaterFixtureType`
3. **Generator equipment** - Not currently parsed
4. **Dehumidifier** - Not currently parsed

---

## Key Implementation Patterns

### Pattern 1: Simple Field Addition
```rust
// Add to Building or Equipment struct
pub new_field: Option<FieldType>,

// Parse in HPXML module
if let Some(val) = child_f64(node, "FieldName") {
    params.insert("new_field".to_string(), json!(val));
}

// Use in simulation
let effective_val = self.new_field.unwrap_or(default_value);
```

### Pattern 2: Enum Extension
```rust
// Extend existing enum
pub enum ExistingEnum {
    ExistingVariant,
    NewVariant { new_field: bool },  // Add field to variant
}

// Update parsing logic
match child_text(node, "Type") {
    Some("Basement") => {
        let conditioned = child_text(node.child("Conditioned"))
            .map(|t| t.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        ExistingEnum::NewVariant { conditioned }
    }
    // ...
}
```

### Pattern 3: Two-Phase ID Resolution
```rust
// Phase 1: Parse all elements with IDs
let mut elements: HashMap<String, Element> = HashMap::new();
for el in parse_elements() {
    elements.insert(el.id.clone(), el);
}

// Phase 2: Resolve references
for el in elements.values_mut() {
    if let Some(ref_id) = &el.reference_id {
        if let Some(target) = elements.get(ref_id) {
            el.resolved_reference = Some(target.clone());
        }
    }
}
```

### Pattern 4: Default Value with Override
```rust
// Define default
const DEFAULT_VALUE: f64 = 0.9;

// Parse with override
pub custom_value: Option<f64>,

// Use with fallback
let value = self.custom_value.unwrap_or(DEFAULT_VALUE);
```

---

## Validation Strategy

For each new field, add validation:

```rust
// In validation.rs
pub fn validate_building_ranges(building: &Building) -> ValidationReport {
    let mut report = ValidationReport::new();
    
    // Range validation
    if let Some(val) = building.new_field {
        if !(MIN..=MAX).contains(&val) {
            report.add_error("new_field", val, MIN, MAX);
        }
    }
    
    // Cross-field validation
    if let (Some(a), Some(b)) = (building.field_a, building.field_b) {
        if a < b {
            report.add_error("field_a must be >= field_b", a, b, f64::MAX);
        }
    }
    
    report
}
```

---

## Testing Checklist

For each implemented field:
- [ ] Parse test with field present
- [ ] Parse test with field absent (uses default)
- [ ] Validation test for out-of-range values
- [ ] Simulation integration test
- [ ] Parity test against OCHRE (if applicable)
- [ ] Documentation update

---

## Notes on OCHRE Compatibility

1. **Fields OCHRE parses but doesn't use**: These are primarily for:
   - Rating calculations (label rates/costs)
   - Future features (natural ventilation)
   - Detailed physics not yet implemented

2. **Fields HARES should implement first**: Focus on fields that:
   - Affect simulation results significantly
   - Are commonly present in HPXML files
   - Enable OCHRE parity tests to pass

3. **Fields to defer**: 
   - Rating-only data
   - Niche equipment (generators, dehumidifiers)
   - Features not in current physics models

---

*Document generated based on analysis of OCHRE v2.x and HARES main branch*
*Last updated: March 2026*
