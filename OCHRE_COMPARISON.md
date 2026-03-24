# HARES vs OCHRE Implementation Comparison

## Summary

HARES has successfully implemented the core ENV ticket functionality and matches most of OCHRE's envelope modeling capabilities. However, several gaps exist, primarily around **adjacent/interior walls**, **film resistance calculations**, and **explicit furniture modeling**.

---

## ✅ Well-Implemented (Matching OCHRE)

### 1. **Foundation/Saleb Handling (ENV-002, ENV-005, ENV-006)**
- ✅ Slabs connect to `Ground` zone (ENV-002)
- ✅ Foundation wall insulation parsing with "Half R{n}" format (ENV-005)
- ✅ Foundation wall area scaling by `depth_below_grade / height` (ENV-005)
- ✅ Slab insulation parsing (perimeter, under-slab, whole slab) (ENV-006)

**Files:**
- `crates/hares-io/src/hpxml/building.rs:448-476` - Post-processing
- `crates/hares-io/src/hpxml/building.rs:876-955` - `extract_foundation_wall_insulation()`
- `crates/hares-io/src/hpxml/building.rs:965-1006` - `extract_slab_insulation()`

### 2. **Window U-Factor Decomposition (ENV-003)**
- ✅ EnergyPlus Simple Window Model Step 1 implemented
- ✅ Proper handling of U-factor → (r_glass, r_film_int) with r_film_ext = 0
- ✅ Piecewise polynomial for interior film resistance
- ✅ Window solar gain with SHGC and angle-of-incidence modifier

**Files:**
- `crates/hares-physics/src/solar.rs:562-575` - `window_u_factor_decomposition()`
- `crates/hares-physics/src/solar.rs:596-665` - `window_shgc_decomposition()`

### 3. **Boundary LUT Mapping (ENV-004)**
- ✅ "Raised Floor" correctly mapped for Conditioned→Outdoor floors
- ✅ All 33 OCHRE boundary types supported

**Files:**
- `crates/hares-io/src/envelope_lut.rs:327`

### 4. **RC Network Construction**
- ✅ Three construction paths: precomputed LUT, raw material layers, fallback R
- ✅ Same-zone boundaries halved (thermal mass only, no heat transfer)
- ✅ Diagnostics capture (ENV-001)

**Files:**
- `crates/hares-envelope/src/boundary_rc.rs:295-406`

### 5. **Infiltration**
- ✅ ASHRAE wind-stack method (quadrature combination)
- ✅ ELA (Effective Leakage Area) method
- ✅ Pressure exponent `n_i` handling (0.5-0.7 range)
- ✅ Terrain class support (Rural/Suburban/Urban)

**Files:**
- `crates/hares-physics/src/infiltration.rs`

### 6. **Natural Ventilation**
- ✅ Operable window area calculation (6.7% of total window area)
- ✅ Stack and wind coefficients
- ✅ Temperature and humidity-based availability

**Files:**
- `crates/hares-physics/src/infiltration.rs:175-210`
- `crates/hares-envelope/src/thermal_solver/config.rs:55-94`

### 7. **Zone Capacitance**
- ✅ Interior mass multiplier (7×) applied to zone air capacitance
- ✅ Volume-based calculation with furniture multiplier

**Files:**
- `crates/hares-envelope/src/boundary_rc.rs:197-209`

---

## ⚠️ Gaps (OCHRE Features Not Yet in HARES)

### 1. **Adjacent/Interior Walls**
**Status:** Partially implemented in LUT but not parsed from HPXML

**OCHRE Capabilities:**
- Adjacent Wall (Indoor↔Indoor)
- Adjacent Ceiling (Indoor↔Indoor)  
- Adjacent Floor (Indoor↔Indoor)
- Adjacent Attic Wall (Attic↔Attic)
- Adjacent Garage Wall (Garage↔Garage)
- Adjacent Foundation Wall (Foundation↔Foundation)
- Adjacent Rim Joist (Foundation↔Foundation)

**HARES Status:**
- LUT supports "Interior Wall" mapping (line 305)
- No explicit parsing of these wall types from HPXML
- OCHRE's `get_boundaries_by_zones()` groups walls by (interior, exterior) zone pairs
- HARES only parses walls from `<Walls>`, `<FoundationWalls>`, etc.

**Impact:** Medium - Missing thermal mass from interior partitions

**Files to compare:**
- OCHRE: `ochre/utils/hpxml.py:87-134` - `get_boundaries_by_zones()`
- OCHRE: `ochre/utils/hpxml.py:302-320` - Wall categorization
- HARES: `crates/hares-io/src/hpxml/building.rs:588-619` - `parse_boundaries()`

### 2. **Explicit Furniture/Interior Mass Boundaries**
**Status:** Not implemented (using lumped multiplier instead)

**OCHRE Capabilities:**
- Indoor Furniture (explicit RC boundary with capacitance)
- Attic Furniture
- Garage Furniture  
- Foundation Furniture

**HARES Status:**
- Uses `INTERIOR_MASS_MULTIPLIER = 7.0` on zone air capacitance
- No explicit furniture boundaries in RC network

**Impact:** Low - Current approach matches total thermal mass, just different modeling approach

**OCHRE Code:**
```python
# Envelope.py:443-448
self.capacitance = self.volume * rho_air * cp_air * capacitance_multiplier  # in kJ/K
# But also creates explicit furniture boundaries with RC networks
```

### 3. **Film Resistance Calculations**
**Status:** Simplified constant values vs. OCHRE's physics-based calculation

**OCHRE Capabilities:**
- DOE-2 model for exterior surfaces (wind-dependent)
- TARP model for interior surfaces (temperature-dependent)
- Surface tilt consideration
- Roughness factor consideration
- Heat transfer coefficient calculation

**HARES Status:**
- Constant values:
  - `R_FILM_EXTERIOR_M2_K_W = 0.03`
  - `R_FILM_INTERIOR_M2_K_W = 0.12`

**Impact:** Low-Medium - May affect accuracy in extreme conditions

**Files to compare:**
- OCHRE: `ochre/utils/envelope.py:342-402` - `calculate_film_resistances()`
- HARES: `crates/hares-envelope/src/boundary_rc.rs:31-34`

**OCHRE Formula (interior):**
```python
if tilt == 90:  # vertical
    h_natural = 1.31 * delta_t ** (1/3)
else:
    # enhanced/reduced based on above/below zones
    h_natural = 9.482 * delta_t ** (1/3) / (7.238 - cos_tilt)  # or
    h_natural = 1.810 * delta_t ** (1/3) / (1.382 + cos_tilt)
```

**OCHRE Formula (exterior):**
```python
h_glass = (h_natural**2 + (3.40 * wind_speed**0.75) ** 2) ** 0.5
h_forced = r_f * (h_glass - h_natural)  # r_f = roughness factor
```

### 4. **Attic/Garage Window Handling**
**Status:** Not implemented (only Indoor windows supported)

**OCHRE Capabilities:**
- Windows in attic walls (`attic_windows`)
- Windows in garage walls (`gar_windows`)
- `get_boundaries_by_wall()` assigns windows to appropriate zones

**HARES Status:**
- Only parses windows from `<Windows>` element
- Creates `Window` boundaries but always attached to walls
- No special handling for attic/garage windows

**Impact:** Low - Attic/garage windows are rare

**Files to compare:**
- OCHRE: `ochre/utils/hpxml.py:356-362`
- HARES: `crates/hares-io/src/hpxml/building.rs:621-700`

### 5. **Duct System Parsing**
**Status:** Basic implementation

**OCHRE Capabilities:**
- Detailed duct location parsing (Interior/Exterior/Attic/Garage/Foundation/Crawlspace)
- Fractional distribution of ducts across locations
- HPXML `Ducts` element parsing

**HARES Status:**
- Has duct parsing in `parse_duct_systems()`
- Need to verify full OCHRE parity

**Files to compare:**
- OCHRE: `ochre/utils/hpxml.py:1029-1127`
- HARES: `crates/hares-io/src/hpxml/building.rs:1512-1545`

### 6. **Interior Radiation/Longwave Exchange**
**Status:** Different implementation approach

**OCHRE Capabilities:**
- View factor calculations between zone surfaces
- Explicit longwave radiation exchange
- Numba-accelerated radiation solver
- Configurable linear/non-linear modes

**HARES Status:**
- Has `longwave_radiation.rs` module
- Different implementation (network-based view factors)

**Impact:** Low - Both models achieve similar net effect through different means

**Files:**
- OCHRE: `ochre/Models/Envelope.py:90-170`
- HARES: `crates/hares-envelope/src/longwave_radiation.rs`

---

## 📊 Detailed Comparison Matrix

| Feature | OCHRE | HARES | Status | Priority |
|---------|-------|-------|--------|----------|
| **Envelope Parsing** | | | | |
| Exterior walls | ✅ | ✅ | Complete | - |
| Attic walls | ✅ | ✅ | Complete | - |
| Garage walls | ✅ | ✅ | Complete | - |
| Attached walls | ✅ | ✅ | Complete | - |
| Adjacent walls | ✅ | ⚠️ | Partial (LUT only) | Medium |
| Foundation walls | ✅ | ✅ | Complete | - |
| Rim joists | ✅ | ✅ | Complete | - |
| Roofs | ✅ | ✅ | Complete | - |
| Floors (raised) | ✅ | ✅ | Complete | - |
| Slabs | ✅ | ✅ | Complete | - |
| Windows | ✅ | ⚠️ | Indoor only | Low |
| Doors | ✅ | ✅ | Complete | - |
| Interior walls | ✅ | ⚠️ | Not parsed | Medium |
| Furniture mass | ✅ (explicit) | ✅ (lumped) | Different approach | Low |
| **RC Construction** | | | | |
| Precomputed LUT | ✅ | ✅ | Complete | - |
| Material layers | ✅ | ✅ | Complete | - |
| Fallback R | ✅ | ✅ | Complete | - |
| Same-zone halving | ✅ | ✅ | Complete | - |
| Diagnostics | ✅ | ✅ | Complete | - |
| **Thermal Properties** | | | | |
| Window U-factor | ✅ | ✅ | Complete | - |
| Window SHGC | ✅ | ✅ | Complete | - |
| Film resistances (constant) | ✅ | ✅ | Complete | - |
| Film resistances (physics) | ✅ | ❌ | Not implemented | Low |
| Insulation parsing | ✅ | ✅ | Complete | - |
| Construction types | ✅ | ✅ | Complete | - |
| Finish types | ✅ | ✅ | Complete | - |
| **Infiltration/Ventilation** | | | | |
| ASHRAE wind-stack | ✅ | ✅ | Complete | - |
| ELA method | ✅ | ✅ | Complete | - |
| Natural ventilation | ✅ | ✅ | Complete | - |
| Forced ventilation | ✅ | ⚠️ | Verify parity | Low |
| **Zones** | | | | |
| Conditioned | ✅ | ✅ | Complete | - |
| Attic | ✅ | ✅ | Complete | - |
| Garage | ✅ | ✅ | Complete | - |
| Foundation | ✅ | ✅ | Complete | - |
| Outdoor | ✅ | ✅ | Complete | - |
| Ground | ✅ | ✅ | Complete | - |
| **Advanced Features** | | | | |
| Linearized infiltration | ✅ | ⚠️ | Verify | Low |
| Linearized radiation | ✅ | ⚠️ | Verify | Low |
| Humidity modeling | ✅ | ✅ | Complete | - |

---

## 🎯 Recommendations

### High Priority (If Energy Balance Errors Observed)
1. **Verify adjacent wall parsing** - Check if HPXML files contain walls with same-zone interior/exterior that should create thermal mass

### Medium Priority (Future Enhancement)
1. **Add explicit adjacent wall parsing** - Implement `get_boundaries_by_zones()` equivalent to catch all OCHRE boundary types
2. **Document film resistance simplification** - Add comments explaining why constant values are used vs. physics-based

### Low Priority (Nice to Have)
1. **Physics-based film resistances** - Implement DOE-2/TARP models for extreme condition accuracy
2. **Explicit furniture boundaries** - If more detailed interior mass modeling is needed
3. **Attic/garage windows** - For completeness

---

## 🔍 Verification Checklist

To verify current HARES implementation against OCHRE:

- [ ] Run BEopt model through both tools and compare total UA
- [ ] Compare zone capacitance values
- [ ] Verify infiltration rates under various wind/temp conditions
- [ ] Check window heat transfer with different U-factors
- [ ] Test slab-ground connections
- [ ] Validate foundation wall area scaling
- [ ] Compare natural ventilation flow rates

---

## References

**OCHRE Key Files:**
- `ochre/utils/hpxml.py` - HPXML parsing (1807 lines)
- `ochre/utils/envelope.py` - Envelope utilities (686 lines)
- `ochre/Models/Envelope.py` - Envelope model (1465 lines)
- `ochre/defaults/Envelope/Envelope Boundaries.csv` - 33 boundary types
- `ochre/defaults/Envelope/Envelope Boundary Types.csv` - Construction mappings
- `ochre/defaults/Envelope/Envelope Materials.csv` - Material properties

**HARES Key Files:**
- `crates/hares-io/src/hpxml/building.rs` - HPXML parsing
- `crates/hares-io/src/envelope_lut.rs` - LUT mapping
- `crates/hares-envelope/src/boundary_rc.rs` - RC construction
- `crates/hares-physics/src/solar.rs` - Window calculations
- `crates/hares-physics/src/infiltration.rs` - Infiltration models

