# HARES vs OCHRE Thermal Model Discrepancy Debugging Plan

## Executive Summary

The oracle tests reveal **critical discrepancies** in HARES thermal physics:

| Metric | OCHRE | HARES | Deviation | Severity |
|--------|-------|-------|-----------|----------|
| **Indoor infiltration** | -11.7 W | **-339.2 W** | **+2799%** | CRITICAL |
| **Window solar gain** | 356.1 W | **141.2 W** | **-60%** | CRITICAL |
| **Heater energy** | 0.91 kWh | **3.27 kWh** | **+259%** | HIGH |
| **Zone conduction gains** | Various | **0 W (all)** | Missing | HIGH |

The 28x infiltration error is the root cause of excessive heating energy. The missing component outputs prevent granular diagnosis.

---

## Phase 1: Infiltration Discrepancy Investigation (HIGHEST PRIORITY)

### 1.1 Understand the Discrepancy

**Problem**: HARES infiltration is 28x higher than OCHRE

**Location**: `crates/hares-envelope/src/thermal_solver/infiltration.rs`
**Configuration**: `crates/hares-core/src/dwelling/solver_builder.rs:349-458`

**Current HARES approach for conditioned zone:**
- Uses AIM-2 model from ACH50 (`aim2_coefficients_from_ach50`)
- Converts ACH50 → flow coefficient C → stack/wind coefficients
- Accounts for foundation type, shielding, terrain, flue

**OCHRE approach (from oracle comments):**
- Uses same AIM-2 model (Walker-Wilson 1998)
- Should produce similar results

### 1.2 Diagnostic Tasks

1. **Extract runtime infiltration parameters**
   - Add debug output to print actual `c_s`, `c_w`, `shelter_coeff`, `n_i` values
   - Verify these match expected values for BEopt building

2. **Verify ACH50 source value**
   - Check what ACH50 value is being used from HPXML
   - Compare to OCHRE's expected value
   - Check if ACH50→natural ACH conversion is correct

3. **Check infiltration height calculation**
   - Current: `default_ceiling_height_m * floors_above_grade`
   - Should use actual building height for stack effect
   - Verify against OCHRE's calculation

4. **Validate environmental conditions**
   - Print `delta_t_c` and `wind_speed_m_s` at runtime
   - Compare to OCHRE reference conditions

5. **Foundation type verification**
   - Check if `has_vented_crawlspace()` returns correct value
   - Foundation type affects wind factor calculation

### 1.3 Implementation: Add Infiltration Diagnostics

**File**: `crates/hares-envelope/src/thermal_solver/mod.rs`

Add diagnostic tracking for:
- Per-zone infiltration method configuration
- Runtime `q_inf_m3_s` values
- `h_inf` conductance values
- Delta-T and wind speed used

**File**: `crates/hares-envelope/src/thermal_solver/config.rs`

Add to `EnvelopeComponentGains`:
```rust
pub infiltration_flow_m3_s: f64,      // Volumetric flow rate
pub infiltration_delta_t_c: f64,      // Temperature difference used
pub infiltration_wind_speed_m_s: f64, // Wind speed used
```

### 1.4 Cross-Reference with OCHRE

**Action**: Compare HARES calculated coefficients against OCHRE:
- Run OCHRE BEopt example and extract infiltration coefficients
- Compare `c_s`, `c_w`, `shelter_coeff` values
- Identify any constant factor differences

---

## Phase 2: Window Solar Gain Discrepancy Investigation

### 2.1 Problem Analysis

**Issue**: HARES window solar 60% lower than OCHRE (141W vs 356W)

**Location**: `crates/hares-envelope/src/thermal_solver/solar.rs`
**Configuration**: `solver_builder.rs:315-346`

**Current calculation:**
```rust
let poa_beam = irr.direct_w_m2 * iam_beam;
let poa_diffuse = irr.diffuse_w_m2 * iam_diffuse;
let transmitted_beam_w = win.area_m2 * win.transmittance * poa_beam;
let transmitted_diffuse_w = win.area_m2 * win.transmittance * poa_diffuse;
```

### 2.2 Diagnostic Tasks

1. **Verify window properties**
   - SHGC: Check value from HPXML vs what OCHRE uses
   - Transmittance: Verify calculation in `calculate_window_parameters`
   - Area: Compare total window area (15.61 m² expected)

2. **Check solar irradiance values**
   - Compare `irr.direct_w_m2` and `irr.diffuse_w_m2` against OCHRE
   - Verify angle of incidence calculations
   - Check IAM (incidence angle modifier) values

3. **Examine interior distribution**
   - Current code distributes solar to interior surfaces (60% beam to floors)
   - May be losing solar if no interior surfaces configured
   - Check if distribution is working correctly

4. **Add solar diagnostics**
   - Track per-window contributions
   - Log POA irradiance values
   - Track IAM factors

### 2.3 Implementation: Add Solar Diagnostics

**File**: `crates/hares-envelope/src/thermal_solver/config.rs`

Add to `EnvelopeComponentGains`:
```rust
pub window_poa_irradiance_w_m2: f64,  // Plane of array irradiance
pub window_iam_beam: f64,              // Beam IAM factor
pub window_iam_diffuse: f64,           // Diffuse IAM factor
pub window_total_area_m2: f64,         // Total window area
```

---

## Phase 3: Missing Component Output Investigation

### 3.1 Problem Analysis

**Issue**: All zone-level conduction gains report 0W:
- Wall Heat Gain - Indoor: 0W (expected ~-345W)
- Roof Heat Gain - Indoor: 0W (expected ~-171W)  
- Floor Heat Gain - Indoor: 0W (expected ~-759W)
- Window Heat Gain - Indoor: 0W (expected ~-53W)
- Radiation Heat Gain - Indoor: 0W (expected ~96W)

**Root Cause**: These are likely being calculated but not properly accumulated into `EnvelopeComponentGains`.

### 3.2 Investigation Tasks

1. **Trace heat flow paths**
   - Wall/roof/floor conduction goes through RC network
   - Check if solver is tracking these separately
   - May need to add per-boundary tracking

2. **Check output wiring**
   - Verify `EnvelopeComponentGains` accumulation in `thermal_solver/mod.rs`
   - Ensure gains are captured before being cleared each step

3. **Add boundary-level tracking**
   - Track per-boundary heat flows
   - Categorize by boundary type (wall, roof, floor, window)

### 3.3 Implementation: Add Component Breakdown

**File**: `crates/hares-envelope/src/thermal_solver/config.rs`

Add detailed component gains:
```rust
pub wall_conduction_w: f64,
pub roof_conduction_w: f64,
pub floor_conduction_w: f64,
pub window_conduction_w: f64,
pub interior_lwr_detailed_w: f64,
```

---

## Phase 4: Implementation Plan

### Ticket 1: Infiltration Diagnostics Framework
**Priority**: CRITICAL
**Files**:
- `crates/hares-envelope/src/thermal_solver/config.rs`
- `crates/hares-envelope/src/thermal_solver/mod.rs`
- `crates/hares-envelope/src/thermal_solver/infiltration.rs`

**Tasks**:
1. Add infiltration diagnostic fields to `EnvelopeComponentGains`
2. Track `q_inf_m3_s`, `h_inf`, `delta_t`, `wind_speed` per zone
3. Add debug logging option for infiltration calculations
4. Export infiltration method parameters (c_s, c_w, shelter_coeff)

### Ticket 2: Solar Gain Diagnostics Framework
**Priority**: CRITICAL
**Files**:
- `crates/hares-envelope/src/thermal_solver/config.rs`
- `crates/hares-envelope/src/thermal_solver/solar.rs`

**Tasks**:
1. Add solar diagnostic fields to `EnvelopeComponentGains`
2. Track per-window POA irradiance and IAM factors
3. Track interior distribution fractions
4. Add debug logging for solar calculations

### Ticket 3: Fix Missing Component Outputs
**Priority**: HIGH
**Files**:
- `crates/hares-envelope/src/thermal_solver/mod.rs`
- `crates/hares-envelope/src/thermal_solver/config.rs`

**Tasks**:
1. Add per-boundary heat flow tracking
2. Categorize flows by boundary type
3. Accumulate into `EnvelopeComponentGains`
4. Update output columns in dwelling

### Ticket 4: Cross-Validation Tool
**Priority**: HIGH
**Files**:
- `tests/envelope_oracle.rs`
- `tests/freefloat_oracle.rs`

**Tasks**:
1. Add detailed comparison table for all physical contributions
2. Create side-by-side OCHRE vs HARES breakdown
3. Add assertions for physical reasonableness (bounds checking)
4. Export comparison data for analysis

---

## Phase 5: Root Cause Hypotheses

### Hypothesis 1: Infiltration ACH50 Value Wrong
**Likelihood**: HIGH
**Test**: Compare HPXML ACH50 value with OCHRE's expected value
**Evidence**: If ACH50 is ~10x higher than expected, explains 28x flow difference (due to n_i=0.65 exponent)

### Hypothesis 2: Window SHGC/Transmittance Miscalculation  
**Likelihood**: MEDIUM
**Test**: Compare window properties calculation against OCHRE
**Evidence**: 60% solar deficit could come from wrong SHGC or missing inward-flowing fraction

### Hypothesis 3: Missing Interior LWR Exchange
**Likelihood**: MEDIUM
**Test**: Check if interior surfaces are configured for LWR
**Evidence**: Interior radiation shows 0W, but should be ~96W

### Hypothesis 4: Component Gains Not Being Accumulated
**Likelihood**: HIGH
**Test**: Trace heat flows through RC network
**Evidence**: All conduction gains show exactly 0W (not just small values)

---

## Next Immediate Actions

1. **Add infiltration debug output** - Print actual coefficients and flows
2. **Run with detailed logging** - Capture all thermal contributions step-by-step
3. **Compare coefficients with OCHRE** - Verify AIM-2 parameters match
4. **Verify ACH50 source** - Check HPXML parsing is correct

## Success Criteria

- Infiltration within 50% of OCHRE (currently 2799% off)
- Window solar within 20% of OCHRE (currently 60% off)
- Component gains visible and comparable to OCHRE
- Heater energy within 30% of OCHRE (currently 259% off)
