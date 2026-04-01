# Dual-Stage Gas Furnace Review - HARES

**Review Date**: March 31, 2026  
**Reviewer**: Code Reviewer  
**Scope**: Dual-Stage (Two-Speed) Gas Furnace configuration in HARES

---

## Executive Summary

This review analyzes the Dual-Stage Gas Furnace implementation in HARES, comparing against OCHRE's implementation and assessing HPXML parsing, default/fallback values, wiring, control logic, telemetry, and thermal solver integration.

**Overall Assessment**: HARES has **incomplete support** for dual-stage gas furnaces. While the underlying HVAC infrastructure (staging.rs, speed_control.rs) supports multi-speed equipment, the gas furnace configuration (`GasFurnaceConfig`) does not expose typed fields for stage capacities and stage EIRs. Additionally, the multispeed defaults CSV has no entries for Gas Furnace, meaning dual-stage furnaces cannot be properly configured from HPXML without manual typed config. The HPXML parsing also lacks support for reading furnace stage information from the HPXML schema.

---

## 1. HPXML Parsing

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-io/src/hpxml/resolve_hvac.rs` - Lines 400-514
- `/home/rich/src/HARES/vendors/OCHRE/ochre/utils/hpxml.py` - Lines 860-876

### HPXML Attributes Parsed for Gas Furnace

| HPXML Attribute | HARES Parameter | Parsing Function | Status |
|-----------------|-----------------|------------------|--------|
| `HeatingCapacity` | `capacity_w` | Direct parsing | Parsed |
| `AnnualHeatingEfficiency/Units=AFUE` | `afue` | Direct parsing | Parsed |
| `extension/FanPowerWattsPerCFM` | `fan_power_w` | Extension parsing | Parsed |
| `extension/FanPowerWatts` | `fan_power_w` | Extension parsing | Parsed |
| `FractionHeatLoadServed` | `fraction_load_served` | Direct parsing | Parsed |
| Duct info | DuctConfig | ASHRAE 152 computation | Parsed |
| **CompressorType (for stages)** | Not parsed | N/A | **NOT SUPPORTED** |

### Key Finding: No Stage Information in HPXML

HPXML does not have a standard field for specifying furnace stage information. Unlike cooling systems which have `<CompressorType>` ("single stage", "two stage", "variable speed"), heating systems with `<Furnace>` type have no equivalent field in the HPXML schema.

**HPXML Parsing Logic** (resolve_hvac.rs:400-406):
```rust
fn n_speeds_from_params(params: &Map<String, Value>) -> u8 {
    params
        .get("number_of_speeds")
        .and_then(Value::as_u64)
        .unwrap_or(1) as u8  // Defaults to 1
}
```

The `number_of_speeds` must be explicitly provided in typed config - it's not derivable from standard HPXML for furnaces.

### What is Missing

1. **Critical**: No HPXML attribute exists for furnace staging in the HPXML standard
2. **Critical**: No mechanism to specify low-stage capacity ratio in HPXML
3. **Critical**: No mechanism to specify different efficiency per stage in HPXML

---

## 2. OCHRE Defaults

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/staging.rs` - Lines 12-17
- `/home/rich/src/HARES/defaults/HVAC Multispeed Parameters.csv`
- `/home/rich/src/HARES/vendors/OCHRE/ochre/defaults/HVAC Multispeed Parameters.csv`

### Default Values for Dual-Stage

| Parameter | HARES Default | OCHRE Default | Source | Assessment |
|-----------|---------------|---------------|--------|------------|
| `number_of_speeds` | 1 (from config) | 1 (defaults) | Config | Can be overridden |
| `low_speed_capacity_fraction` | 0.5 | N/A (not used for furnaces) | HARES staging.rs:13 | **HARES-specific** |
| `min_time_per_speed_s` | From config | 5 minutes (300s) | DynamicHVAC defaults | Configurable |
| `PLF degradation coefficient` | 0.25 | 0.25 | AHRI 210/240 S6.6.3 | Matches OCHRE |

### OCHRE Multispeed Parameters CSV Analysis

The multispeed parameters CSV files have **NO entries for Gas Furnace**:

```
HVAC Name,HVAC Efficiency,Number of Speeds,Capacity Ratio 1,...
ASHP Cooler,16.0 SEER,2,0.72,...
ASHP Heater,8.7 HSPF,2,0.72,...
MSHP Cooler,13.0 SEER,4,0.48889,...
(Gas Furnace - NO ENTRIES)
```

**Impact**: When `number_of_speeds > 1` is set for a Gas Furnace, there are no default capacity ratios or COP values to populate stage_capacities_w and stage_eirs. The code silently falls back to single-speed behavior.

### HARES Staging Defaults (staging.rs)

```rust
/// Default low-speed capacity fraction for two-speed equipment.
pub(super) const DEFAULT_LOW_SPEED_CAPACITY_FRACTION: f64 = 0.5;

/// Default part-load factor degradation coefficient (Cd).
/// AHRI Standard 210/240-2023, S6.6.3 default when no test data available.
pub(super) const DEFAULT_PLF_DEGRADATION_COEFF: f64 = 0.25;
```

---

## 3. HARES Wiring

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heating_config.rs` - Lines 10-45
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 44-56, 200-280

### Config Structure

```rust
// heating_config.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasFurnaceConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub capacity_w: f64,              // Required - single value only
    pub afue: f64,                    // Required - single value only
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,         // Can be set to 2
    #[serde(flatten)]
    pub ducts: DuctConfig,
    // MISSING: stage_capacities_w: Option<Vec<f64>>
    // MISSING: stage_eirs: Option<Vec<f64>>
    // MISSING: stage_shrs: Option<Vec<f64>>  (not applicable for heating)
}
```

### Critical Gap: No Stage Parameters in Config

The `GasFurnaceConfig` lacks fields for:
- `stage_capacities_w` - Per-stage heating capacities (e.g., [6000, 12000])
- `stage_eirs` - Per-stage energy input ratios (e.g., [0.35, 0.38] for different COP at each stage)
- `stage_control_mode` - Which two-speed control algorithm to use

Compare to AC config (ac_config.rs):
```rust
pub struct AirConditionerConfig {
    // ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_capacities_w: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_eirs: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_shrs: Option<Vec<f64>>,
}
```

### Equipment Initialization (furnace.rs:200-280)

The furnace initialization uses `capacity_w` as a single value:
```rust
self.hvac.heating_capacities_w = if typed.number_of_speeds > 1 {
    // THIS PATH IS NOT IMPLEMENTED FOR FURNACE
    // No stage_capacities_w field to use
    vec![typed.capacity_w; typed.number_of_speeds as usize]
} else {
    vec![typed.capacity_w]
};
```

**When `number_of_speeds > 1` is set but no stage capacities are provided**, the code creates a vector of identical capacities, effectively making it behave like a single-stage furnace with more stages.

---

## 4. Control Logic

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/staging.rs` - Lines 68-201
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/speed_control.rs`

### Two-Speed Control Modes

HARES implements three two-speed control algorithms in staging.rs:

#### 1. TwoSpeedSetpoint (Lines 123-149)
```rust
fn select_two_speed_setpoint(&mut self, load_fraction: f64) -> SpeedSelection {
    let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
    let desired_index = if load_fraction > low_cap { 1 } else { 0 };
    // If load_fraction > 0.5 (default low_cap), use high stage
    // Otherwise use low stage
}
```
**Logic**: If desired capacity > 50% of max, use high stage; otherwise use low stage.

#### 2. TwoSpeedTime (Lines 152-201)
```rust
fn select_two_speed_time(&mut self, load_fraction: f64, zone_temp_c: Option<f64>, is_heating: bool) -> SpeedSelection {
    // If temperature moving "wrong direction" (dropping during heating)
    // after min_time_per_speed_s has elapsed, escalate to high stage
}
```
**Logic**: Uses temperature direction change - starts at low stage, escalates to high if temperature continues to move in wrong direction after minimum time. Matches OCHRE's "Time" control type.

#### 3. TwoSpeedAlternating (Lines 91-109)
```rust
// Simple alternating: if high stage is enabled, always use it
```
**Logic**: Uses high stage when available.

### Control Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `low_speed_capacity_fraction` | 0.5 (50%) | Load fraction threshold for stage transition |
| `min_time_per_speed_s` | Configurable | Minimum time at each speed before transition allowed |
| `speed_control_mode` | Not exposed for furnace | Which algorithm to use |

### OCHRE Comparison

OCHRE's DynamicHVAC (HVAC.py:859-916) implements:
- `control_type = "Time"` - Temperature direction based staging
- `control_type = "Setpoint"` - Setpoint difference based staging (overlapping deadband)
- `control_type = "Time2"` - Old time-based (always goes to high)

HARES's implementation is similar but:
- **TwoSpeedSetpoint** matches OCHRE's "Setpoint" control
- **TwoSpeedTime** matches OCHRE's "Time" control

---

## 5. Output Ports & Telemetry

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 380-420, 489-510

### Port Declarations

```rust
// furnace.rs:283-289
ports: vec![
    PortDeclaration::fuel(),        // Natural gas input
    PortDeclaration::electrical(),  // Fan power only
    PortDeclaration::thermal(zone), // Delivered heat to zone
],
```

### Telemetry Fields

| Telemetry Key | Unit | Description | Status |
|---------------|------|-------------|--------|
| `electric_kw` | kW | Fan-only electric power | Available |
| `fan_kw` | kW | Fan electric power (alias) | Available |
| `fuel_input_w` | W | Fuel input power | Available |
| `thermal_output_w` | W | Delivered heat (post-DSE) | Available |
| `operating_mode` | enum | 0=Off, 1=Heating | Available |
| `supply_air_temp_c` | C | Supply air temperature | Available |
| `heating_setpoint_c` | C | Active heating setpoint | Available |
| `cooling_setpoint_c` | C | Active cooling setpoint | Available |
| **SPEED_INDEX** | index | Current speed stage | **NOT AVAILABLE FOR FURNACE** |
| **runtime_by_stage** | s | Runtime per stage | **NOT AVAILABLE** |

### Missing Telemetry

1. **SPEED_INDEX**: Unlike ASHP and AC which report `SPEED_INDEX` telemetry, the gas furnace does not report which stage is active
2. **Runtime by stage**: No tracking of time spent in each stage

---

## 6. Dwelling Integration

### Files Analyzed
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs` - Lines 350-395
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/staging.rs` - Lines 68-121

### Thermal Solver Integration

The furnace integrates with the thermal solver through zone thermal ports:

```rust
// furnace.rs:376-387
let fan_heat_w = fan_kw * 1000.0;
let total_sensible_w = gross_capacity_w + fan_heat_w;

if total_sensible_w > 0.0 {
    self.hvac.write_zone_thermal_contributions(
        ports,
        total_sensible_w,
        0.0,
        ThermalCategory::HvacHeating,
    )?;
}
```

### Multi-Stage Capacity Handling

When multi-stage is properly configured, the thermal solver would use different capacity levels based on the stage selection:

```rust
// staging.rs:select_speed()
match self.speed_control_mode {
    // ...
    SpeedControlMode::TwoSpeedSetpoint => self.select_two_speed_setpoint(load_fraction),
    SpeedControlMode::TwoSpeedTime => self.select_two_speed_time(...),
    // ...
}
```

The capacity selection determines which `heating_capacities_w` element is used, affecting the thermal energy delivered to the zone.

### Energy Conservation

Even with dual-stage, energy conservation is preserved:
- **Fuel input** = stage_capacity / AFUE (per-stage efficiency would need to be specified)
- **Thermal to zones** = stage_capacity * DSE
- **Duct losses** = stage_capacity * (1 - DSE)

---

## Issues and Findings

### Issue 1 (Critical): No Stage Capacity/EIR Fields in GasFurnaceConfig

**Files**:
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/heating_config.rs:10-45`
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs:200-230`

**Evidence**:
`GasFurnaceConfig` only has `capacity_w: f64` and `afue: f64` - no per-stage fields.

Compare to AC config which has `stage_capacities_w`, `stage_eirs`, `stage_shrs`.

**Impact**: Even if `number_of_speeds` is set to 2 in typed config, there's no way to specify different capacities for low/high stages or different efficiency at each stage. The equipment falls back to identical capacities for all stages.

**Recommended Fix**: Add the following to `GasFurnaceConfig`:
```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub stage_heating_capacities_w: Option<Vec<f64>>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub stage_heating_eirs: Option<Vec<f64>>,
```

And update furnace.rs initialization to use these fields when present.

---

### Issue 2 (Critical): No Gas Furnace Entries in Multispeed Parameters CSV

**Files**:
- `/home/rich/src/HARES/defaults/HVAC Multispeed Parameters.csv`
- `/home/rich/src/HARES/vendors/OCHRE/ochre/defaults/HVAC Multispeed Parameters.csv`

**Evidence**: The multispeed CSV has entries for:
- ASHP Cooler (various SEER ratings, 2-speed and 4-speed)
- ASHP Heater (various HSPF ratings, 2-speed and 4-speed)
- MSHP Cooler (various SEER ratings, 4-speed)
- MSHP Heater (various HSPF ratings, 4-speed)

**No entries for Gas Furnace at any speed count.**

**Impact**: When parsing HPXML with dual-stage furnace, there are no default capacity ratios or COP values to populate stage parameters. The resolver silently skips stage parameter population (resolve_hvac.rs:1872-1876 returns early).

**Recommended Fix**: Add default entries to multispeed CSV, e.g.:
```
Gas Furnace,80 AFUE,2,0.65,0.80,3.33,1,0.8,Manual
Gas Furnace,90 AFUE,2,0.65,0.80,3.75,1,0.9,Manual
```
Typical dual-stage furnaces have low-stage at ~65% of high-stage capacity.

---

### Issue 3 (High Severity): HPXML Has No Furnace Stage Field

**Files**:
- HPXML 3.0 schema / documentation

**Evidence**: HPXML defines `<CompressorType>` for cooling systems:
```xml
<CoolingSystem>
  <CompressorType>two stage</CompressorType>
</CoolingSystem>
```

But there's no equivalent field in `<HeatingSystem>` for furnaces:
```xml
<HeatingSystem>
  <HeatingSystemType>
    <Furnace/>
  </HeatingSystemType>
  <!-- No Stage field available -->
</HeatingSystem>
```

**Impact**: Users cannot specify dual-stage furnaces through standard HPXML files. Must use typed config override.

**Recommended Fix**: Document that dual-stage furnaces require typed config. Consider adding HPXML extension field support if needed.

---

### Issue 4 (Medium Severity): No Stage Runtime Telemetry for Furnace

**Files**:
- `/home/rich/src/HARES/crates/hares-equipment/src/hvac/furnace.rs:489-510`

**Evidence**: The gas furnace telemetry does not include `SPEED_INDEX` or runtime-by-stage tracking, unlike ASHP and AC equipment.

**Impact**: Cannot verify or analyze dual-stage staging behavior from telemetry outputs.

**Recommended Fix**: Add speed index telemetry similar to AC implementation.

---

### Issue 5 (Low Severity): Startup Capacity Degradation Applied to Furnaces

**Note**: This issue was identified in the single-stage furnace review and applies here as well.

**Files**:
- `/home/rich/src/HARES/crates/hares-io/src/hpxml/resolve_hvac.rs:309`

OCHRE only applies startup capacity degradation to heat pumps and cooling equipment. HARES incorrectly applies it to gas furnaces.

---

## Summary

| Aspect | HARES vs OCHRE | Assessment |
|--------|----------------|------------|
| HPXML parsing | No stage info in HPXML for furnaces | Gap - can't specify via HPXML |
| Default values | No multispeed CSV entries for gas furnace | **BUG** - no defaults for 2-stage |
| Control logic | Three two-speed algorithms implemented | Good - infrastructure exists |
| Config structure | Missing stage capacity/eir fields | **BUG** - cannot configure |
| Telemetry | No speed index reporting | Gap |
| Thermal solver | Supports multi-capacity | Good - ready when properly configured |

---

## Where HARES Physics is Better Than OCHRE

1. **Explicit staging algorithms**: Clear separation between Setpoint/Time/Alternating control modes
2. **Typed config validation**: Configuration errors caught at init
3. **Default low-speed fraction**: Explicit 50% default (vs implicit in OCHRE)

---

## Where OCHRE Has Better Defaults (Documented)

1. **Multispeed parameters**: OCHRE has the same gap - no Gas Furnace entries in multispeed CSV
2. **Stage detection**: OCHRE also cannot detect dual-stage from standard HPXML for furnaces

---

## Recommendations

1. **High Priority**: Add `stage_heating_capacities_w` and `stage_heating_eirs` fields to `GasFurnaceConfig`
2. **High Priority**: Add default entries to multispeed CSV for Gas Furnace at 2-speed
3. **Medium Priority**: Add speed index telemetry to gas furnace
4. **Low Priority**: Document typed config requirement for dual-stage furnaces

---

## Conclusion

The dual-stage gas furnace implementation in HARES has the **foundational infrastructure** (staging algorithms, speed control modes) but lacks the **configuration surface** to properly specify and use multi-stage operation. The gas furnace config does not expose stage parameters, and no default multispeed values exist. This results in dual-stage furnaces falling back to single-stage behavior even when `number_of_speeds > 1` is specified.

**Recommendation**: Implement the recommended fixes to enable proper dual-stage furnace support. The underlying staging code is well-designed and matches OCHRE behavior.
