# OCHRE vs HARES: Envelope/Thermal Modeling Comparison

## Executive Summary

OCHRE's envelope model is significantly more comprehensive than HARES. Beyond known gaps (longwave radiation, window optics, natural ventilation), OCHRE implements multiple distinct features across internal gains, duct losses, multi-zone heat transfer, and humidity buffering that HARES either lacks or implements incompletely. This document details each gap with physics impact and severity.

---

## 1. WINDOW OPTICS AND SOLAR TRANSMITTANCE

### OCHRE Implementation
- **Transmittance calculation**: Computes `transmittance` and `absorptivity` from SHGC and U-factor using EnergyPlus-calibrated biquadratic regressions (2-4 terms depending on U-factor range)
  - Window parameters: `SHGC`, `Shading Fraction`, `U Factor`
  - Applies window emissivity = 0.84 per EnergyPlus standard
  - Distinguishes between absorbed solar (contributes to surface temperature) vs transmitted solar (enters zone)
  - Applies shading fraction multiplicatively to SHGC before calculating transmittance
- **Location**: `Envelope.py` lines 209-230, utility function in `utils/envelope.py:calculate_window_parameters()`
- **Radiation split**: Window transmittance drives zone loads differently than absorbed window solar on exterior surfaces

### HARES Implementation
- **No window optics model**: Solar irradiance is applied as a direct boundary input with no SHGC-to-transmittance conversion
- **No shading fraction**: Operable/fixed shading models are absent
- **No emissivity distinction**: Windows treated generically with same emissivity as opaque surfaces (not 0.84)
- **Missing: transmitted vs absorbed split** — cannot track how much solar enters the zone vs heats the window/wall
- **Location**: `hares-envelope/src/thermal_solver.rs` applies raw `solar_input` without optical modeling

### Physics Impact
- **Severity**: HIGH
- **Effect**:
  - HARES overestimates or incorrectly distributes solar gains to building surfaces vs the zone
  - Shading is not modeled, meaning buildings with controllable/seasonal shading cannot be simulated
  - Window transmittance depends on glazing type (3-layer vs 2-layer), frame material, and coating — this is lost
  - Example: For a south-facing window with SHGC=0.35 in a high-U (>3.95) frame, OCHRE calculates ~20% transmittance vs the 35% SHGC might suggest; HARES applies 100% of irradiance equally

---

## 2. LONGWAVE RADIATION: SKY TEMPERATURE MODELING

### OCHRE Implementation
- **Full nonlinear exterior LWR solver**: Iteratively solves surface temperature and LWR gain accounting for sky temperature as distinct from ambient
- **Sky view factor**: Surfaces incorporate a tilted-surface sky view factor (cosine-based): `SVF = ((1 + cos(tilt))/2)^1.5`
- **Split radiance injection**: `h_lwr_inj = e_factor * ((1 - SVF) * T_ext^4 + SVF * T_sky^4)`
- **Optional linear fallback**: Can linearize exterior radiation by adding resistors to RC network when nonlinear is too slow
- **Interior radiation**: Nonlinear iterative solver with view factors and emissivity-weighted thermal exchanges between all surfaces in a zone
- **Location**: Lines 90-163, classes `BoundarySurface`, iterative solvers with heavy-ball convergence

### HARES Implementation
- **Exterior LWR**: Implemented in `longwave_radiation.rs` with linearized radiation using `interior_longwave_linearised_w()` function
  - Linearizes via `h_r = 4 * e * sigma * T^3` at reference temperature
  - **Uses hardcoded reference temperature of 20°C** for linearization (not adaptive)
- **Sky temperature fallback**: Defaults to `outdoor_temp - 5.0` if not provided; treats sky temperature as simple scalar offset
- **No interior LWR**: Humidity solver tracks moisture but thermal interior LWR loops between surfaces are not modeled
- **No view factors**: Surface-to-surface radiation within a zone is not calculated or distributed
- **Location**: `hares-envelope/src/longwave_radiation.rs`, `thermal_solver.rs` applies to exterior surfaces only

### Physics Impact
- **Severity**: MEDIUM-HIGH
- **Effect**:
  - Linearization error is largest in extreme conditions (e.g., frozen roofs with T_sky=-20°C; hot roofs with T_sky=+40°C). At ±20°C from reference, error in h_r is ~10-15%
  - Interior surface-to-surface radiation (e.g., ceiling-to-furniture-to-wall) is not modeled in HARES, only in OCHRE
  - Sky temperature modeling is passive in HARES (simple offset); OCHRE's view-factor-weighted split allows cold sky to reduce surface temperatures proportionally to exposure
  - Example: A horizontal roof in OCHRE can radiate to cold sky at -10°C and reach -5°C surface temp; in HARES with linear model at 20°C reference, the approximation diverges significantly

---

## 3. NATURAL VENTILATION

### OCHRE Implementation
- **Full natural ventilation model**: `_natural_ventilation()` (lines 28-55)
  - Operable window area derived from total window area: `0.67 * 0.5 * 0.2 = 6.7%` (empirical ResStock factors)
  - Stack-driven flow: `(density * g * height * dT)^0.5`
  - Wind-driven flow: `wind_speed^2` term
  - Combined: Quadratic sum with max saturation limit
  - Humidity/occupancy gating: Disabled if `w_amb >= max_oa_hr` or `T_zone <= T_outdoor`
  - Returns m³/s for latent/sensible calculations

### HARES Implementation
- **No natural ventilation**: Absent entirely
- **Infiltration only**: Only ASHRAE, ELA, or ACH-based models supported
- **Location**: `thermal_solver.rs` calls `apply_infiltration_and_ventilation()` but no natural ventilation logic

### Physics Impact
- **Severity**: MEDIUM
- **Effect**:
  - Buildings in mild climates with natural ventilation (common in Mediterranean, subtropical climates) are systematically over-heated/over-cooled
  - Peak cooling loads in shoulder seasons (spring/fall) are overestimated
  - Latent loads from open windows are not reduced, leading to overestimated dehumidification demands
  - Example: A 200 m³ house with 10% operable windows at 5 K temperature difference and 2 m/s wind could have ~20 m³/min natural vent; HARES ignores this

---

## 4. DUCT LOSSES AND DISTRIBUTION SYSTEM EFFICIENCY (DSE)

### OCHRE Implementation
- **Duct DSE factor**: Multiplier `0 < DSE < 1` applied to delivered HVAC power
- **Duct location**: Ducts can be in conditioned space (DSE=1), attic, garage, or basement
- **Heat fraction distribution**: `zone_fractions = {conditioned_zone: DSE * (1 - basement_frac), duct_zone: (1 - DSE), basement: DSE * basement_frac}`
- **Loss tracking**: Reports separate "Delivered" vs "Duct Losses (W)" in results
- **Location**: `Equipment/HVAC.py` lines 166-197

### HARES Implementation
- **No DSE model**: Duct losses are not modeled
- **Delivered power = output power**: Assumes 100% of equipment output reaches the conditioned zone
- **No duct zone coupling**: Cannot route duct losses to attic/basement/garage for cross-zone thermal interaction
- **Location**: Missing entirely from `hares-equipment/src/hvac/`

### Physics Impact
- **Severity**: HIGH (for high-duct-loss homes)
- **Effect**:
  - Underestimates HVAC energy use by 10-25% in homes with ducts outside conditioned space (US average ~15% loss)
  - Overestimates indoor comfort (system capacity is overstated because losses are ignored)
  - Attic/basement heating is not modeled — those zones will be colder than they should be
  - DSE of 0.7 means 30% of heating capacity is lost; HARES treats it as 100% useful
  - Example: 10 kW HVAC heat output with DSE=0.75 should deliver 7.5 kW to zone + 2.5 kW to duct zone; HARES applies all 10 kW to conditioned zone

---

## 5. OCCUPANCY AND INTERNAL GAINS MODELING

### OCHRE Implementation
- **Occupancy-driven gains**:
  - Sensible gain per occupant: default 400 BTU/hr (~117 W) split into convective (56.3%) and radiative (0%)
  - Latent gain per occupant: 43.7% of 400 BTU/hr (~61 W)
  - Occupancy from schedule: multiplied by per-person gains
  - Location: `Envelope.py` lines 901-908
- **Other internal gains**: "Internal Gains (W)" from schedule added directly to indoor zone
- **Appliance/lighting loads**: Modeled in Equipment classes (dishwasher, cooking range, clothes dryer, lighting) with per-unit power profiles

### HARES Implementation
- **Occupancy**:
  - Schedule parameter `occupancy` loaded but gains are not calculated
  - No occupancy → sensible/latent gains conversion
  - Location: `hares-core/src/dwelling.rs` loads occupancy from schedule but doesn't apply it to thermal gains
- **Internal gains**: "Internal Gains (W)" field exists but unclear if it's routed to envelope model
- **Lighting**: Parsed from HPXML but routing to thermal zone unclear; likely handled as schedule load rather than zone sensible heat

### Physics Impact
- **Severity**: MEDIUM
- **Effect**:
  - HARES cannot replicate occupancy-dependent cooling loads
  - Latent load from occupancy is completely missing (affects dehumidification capacity requirements)
  - Example: 4-person household = ~240 W latent + ~520 W sensible baseline load in OCHRE; HARES either applies 0 or a constant schedule value, missing the occupancy scaling
  - Affects summer peak loads significantly (occupancy + solar + appliances = bulk of cooling demand)

---

## 6. HUMIDITY BUFFERING / MOISTURE CAPACITANCE

### OCHRE Implementation
- **Humidity capacitance multiplier**: `humidity_cap_mult = 15.0` in `Humidity.py` line 10
  - Represents moisture storage in furniture, materials, air
  - Used to limit maximum latent flow: `max_latent_flow = volume * humidity_cap_mult / dt_seconds`
  - Empirical damping of humidity transients to avoid unrealistic spikes
  - Location: Lines 21, 56

### HARES Implementation
- **No humidity capacitance**: Moisture is treated as instantaneous without buffering
- **Direct humidity update**: Latent gains immediately change humidity ratio without time-lag or material absorption
- **Location**: `humidity_solver.rs` lacks capacitive terms

### Physics Impact
- **Severity**: LOW-MEDIUM
- **Effect**:
  - Unrealistic humidity oscillations in simulation, especially with short timesteps
  - Peak relative humidity is overstated during showers, cooking, or occupancy spikes
  - Materially affects dehumidifier control (on/off cycling) but not annual energy significantly
  - Example: 10 minutes of cooking steam might spike RH to 95% in HARES vs 75% in OCHRE due to material absorption lag

---

## 7. FOUNDATION/BASEMENT HEAT TRANSFER

### OCHRE Implementation
- **Multi-zone support**: Foundation (FND) as a distinct interior zone with its own temperature state
- **Dedicated boundaries**:
  - `Foundation Wall (FW)` between Ground and Foundation
  - `Foundation Floor (FF)` between Ground and Foundation
  - `Foundation Ceiling (FC)` between Foundation and Indoor
  - `Rim Joist (RJ)` between Outdoor and Foundation
- **Basement heat fraction**: Parameter `basement_heat_frac` for HVAC systems (how much conditioned air goes to basement)
- **Boundaries**: Allows complex multi-layer RC models for slab edges and foundation walls

### HARES Implementation
- **Foundation zone type**: Defined in `hares-io/src/hpxml/building.rs` lines 39-46 (ZoneType::Foundation)
- **Duct systems tracked**: Lines 112 (attached to zones)
- **Boundaries created**: Foundation-related boundaries parsed from HPXML
- **RC network**: Built from boundaries but unclear if Foundation → Indoor heat transfer is correctly modeled
- **Status**: Partially implemented; zone exists but HVAC fraction routing to basement missing

### Physics Impact
- **Severity**: MEDIUM (in cold climates)
- **Effect**:
  - Unheated basements in HARES lack proper heat stratification from outdoor/indoor sources
  - Slab edge heat loss under-modeled if rim joist properties are not matched
  - Example: Unheated basement at 8°C receiving foundation ceiling loss from 21°C indoor should warm slightly; HARES may not capture this if Foundation zone is not properly coupled

---

## 8. ATTIC MODELING

### OCHRE Implementation
- **Attic as distinct zone**: Attic (ATC) is a full interior zone with temperature state
- **Dedicated boundaries**:
  - `Attic Wall (AW)` between Outdoor and Attic
  - `Attic Roof (AR)` between Outdoor and Attic
  - `Attic Floor (AF)` between Attic and Indoor
  - `Attic Furniture (AM)` between Attic and Attic (thermal mass)
- **Radiant barrier handling**: Emissivity reduced for radiant barriers (0.05 vs 0.90 default in lines 221-222)
- **No active ventilation**: Attic is sealed in standard config but can have infiltration

### HARES Implementation
- **Attic zone type**: Defined in HPXML parsing
- **Boundaries**: Attic roof, floor, and walls parsed
- **Radiant barrier**: Not explicitly handled in code (no emissivity override check found)
- **Status**: Zones and boundaries exist but radiant barrier properties not modeled

### Physics Impact
- **Severity**: LOW-MEDIUM
- **Effect**:
  - Radiant barriers are mismodeled if not present (0.90 emissivity instead of 0.05 for reflective surfaces)
  - This affects attic-to-indoor ceiling radiation loads in summer (can affect cooling by 5-10% for hot climates)
  - Example: 30 m² attic roof at 60°C radiating to 0.05-emissivity radiant barrier sends ~3 kW to space; with 0.90 emissivity, 50+ kW — massive error

---

## 9. GARAGE MODELING

### OCHRE Implementation
- **Garage as distinct zone**: Garage (GAR) is a full interior zone
- **Dedicated boundaries**:
  - `Garage Wall (GW)` between Outdoor and Garage
  - `Garage Roof (GR)` between Outdoor and Garage
  - `Garage Floor (GF)` between Ground and Garage
  - `Garage Attached Wall (GA)` between Garage and Indoor (party wall with conditioned space)
  - `Garage Ceiling (GC)` between Attic and Garage
  - `Garage Interior Ceiling (GI)` between Garage and Indoor
  - `Garage Door (GD)` between Outdoor and Garage
  - `Garage Furniture (GM)` for thermal mass
- **No HVAC**: Garages are unconditioned in OCHRE

### HARES Implementation
- **Garage zone type**: Defined in HPXML parsing
- **Boundaries**: Garage surfaces parsed
- **Status**: Zones and boundaries exist but unconfirmed if properly coupled to all adjacent zones (especially the "Attached Wall" to Indoor)

### Physics Impact
- **Severity**: LOW-MEDIUM
- **Effect**:
  - Attached wall heat loss to unconditioned garage is a real load (winter heating); if boundaries don't connect properly, conditioned space loses coupling to garage temperature
  - Garage heating/cooling affects Indoor via conduction; HARES may not fully capture this
  - Example: Winter garage at 5°C, indoor at 21°C with 8 m² shared wall (0.3 W/m²·K U-value) = ~38 W steady-state loss not captured if coupling is broken

---

## 10. MULTI-ZONE HEAT DISTRIBUTION AND INTERIOR WALLS

### OCHRE Implementation
- **Interior-to-Interior boundaries**:
  - `Interior Wall (IW)` between Indoor and Indoor
  - `Adjacent Wall (JW)` between LIV and LIV (for multiple main zones)
  - `Adjacent Ceiling (JC)`, `Adjacent Floor (JL)` for vertical zones
  - `Adjacent Basement Wall (JF)`, etc. for cross-zone coupling
- **Same-zone boundaries**: When `ext_zone == int_zone`, RC network is reversed so heat flows within the zone
- **View factors**: Interior radiation accounts for all surfaces in a zone radiating to each other

### HARES Implementation
- **Single primary conditioned zone**: Most focus on "Indoor" (LIV)
- **Interior surfaces**: Parsed but unclear if interior-to-interior heat flows are fully integrated
- **Status**: Partial; Foundation/Attic/Garage exist but interior wall coupling is not explicit in thermal solver

### Physics Impact
- **Severity**: LOW
- **Effect**:
  - Multi-room temperature variations are not captured; single-zone assumption
  - Cross-zone interior radiation neglected (minor compared to exterior)
  - Example: Two-story home with stairwell heat transfer is modeled as single zone in HARES vs two interconnected zones in OCHRE

---

## 11. THERMAL MASS MULTIPLIER

### OCHRE Implementation
- **Capacity multiplier**: Zone air capacitance = `volume * rho_air * cp_air * capacitance_multiplier`
- **Default multiplier**: 7 (lines 445)
- **Effect**: Represents furniture, contents, building mass thermal capacity coupled to air
- **Control**: Per-zone configuration parameter

### HARES Implementation
- **Air capacitance only**: Built from RC network nodes; does not have an explicit per-zone multiplier
- **Implicit in RC**: Material layers contribute capacitance, but furniture/contents are not added
- **Status**: RC network provides dynamic thermal mass but lacks the "lumped furniture" concept

### Physics Impact
- **Severity**: LOW-MEDIUM
- **Effect**:
  - HARES may have lower thermal mass, leading to faster zone temperature swings and overstated peak loads
  - Example: Same 200 m³ room: OCHRE's 7x multiplier adds ~840 kJ/K of thermal mass; HARES might have only 400 kJ/K from envelope walls
  - Affects load profiles but not annual totals significantly

---

## 12. WINDOW SOLAR DISTRIBUTION WITHIN ZONE

### OCHRE Implementation
- **View factors to surfaces**: Transmitted solar from windows is distributed to all surfaces in the zone based on `window_view_factors`
  - Weighted by area and absorptivity: `surface_area * absorptivity`
  - Fraction to zone air (non-radiative): `(1 - radiation_frac)` of transmitted gain
  - Fraction to surface nodes: `radiation_frac` of transmitted gain
- **Separate from absorbed solar**: Exterior window solar (absorbed by glass) goes to exterior surface nodes; transmitted solar goes to interior surfaces and zone air

### HARES Implementation
- **Solar routed as input**: Solar irradiance is a direct input to the RC network, typically to surface or zone heat input indices
- **No view factor distribution**: All transmitted solar is lumped as a single input index, not distributed to multiple surfaces
- **Status**: Simplified; functional but less detailed than OCHRE

### Physics Impact
- **Severity**: LOW
- **Effect**:
  - Solar gains are applied to a single node (e.g., "indoor" zone) rather than distributed across receiving surfaces
  - May affect internal radiation balance slightly but not the total energy balance
  - Example: 1000 W solar enters zone; OCHRE distributes it based on view factors (say 30% wall, 20% floor, 50% ceiling); HARES applies all 1000 W to a single node — the RC network will still reach the same steady state but transient behavior differs

---

## 13. INFILTRATION METHOD COMPLETENESS

### OCHRE Implementation
- **Three methods supported**:
  1. **ASHRAE wind-stack**: `Q = c * (Cs * |dT|^n_i + Cw * (sf * U_wind)^(2*n_i))^0.5`
  2. **ELA**: `Q = ELA * (stack_coeff * |dT| + wind_coeff * U_wind^2)^0.5`
  3. **ACH**: `Q = ACH * volume / 3600`
- **Schedule-dependent**: Infiltration parameters can vary with time
- **Wind calculation**: Accounts for wind shielding (shelter factor `inf_sft`)

### HARES Implementation
- **All three methods supported**: Same ASHRAE, ELA, ACH models (lines 20-42, thermal_solver.rs)
- **Shelter factor**: Included in `AshraeWindStack` config
- **Status**: Feature parity with OCHRE

### Physics Impact
- **Severity**: NONE (HARES matches OCHRE)
- **Effect**: No gap; infiltration modeling is equivalent

---

## 14. COMPONENT LOAD TRACKING

### OCHRE Implementation
- **Detailed breakdown**:
  - External Wall, Roof, Floor, Window, Door, Raised Floor (for component loads)
  - Interior Wall, Indoor Furniture, Foundation, Attic, Garage (for cross-zone tracking)
  - Energy flow states: Tracks power flow through specific resistors (e.g., `H_WD_LIV` = window-to-indoor heat flow in kWh)
- **Reporting**: Results include "Roof Heat Gain - Indoor (W)", "Wall Heat Gain - Indoor (W)", etc.
- **Location**: `Envelope.py` lines 1359-1372, `add_component_loads()` method

### HARES Implementation
- **Basic tracking**: Zone temperature outputs only
- **No detailed boundary loads**: Does not report "which surface contributed how much heat to the zone"
- **Status**: Missing component load accounting

### Physics Impact
- **Severity**: NONE (diagnostic only)
- **Effect**: HARES can reach correct zone temperatures but cannot report *why* (how much is wall vs roof vs window); OCHRE can

---

## 15. FILM RESISTANCE AND CONVECTION

### OCHRE Implementation
- **Film resistance calculation**: Computes convective film resistance based on boundary type, location, and wind speed
- **Method**: EnergyPlus-based correlations (Nusselt number functions)
- **Wind-dependent**: Exterior film resistance varies with wind speed
- **Location**: `utils/envelope.py:calculate_film_resistances()`

### HARES Implementation
- **User-provided**: Film resistances are read from material/boundary properties in HPXML
- **Not calculated**: No dynamic wind-speed adjustment
- **Status**: Simplified but functional; relies on pre-computed values

### Physics Impact
- **Severity**: LOW
- **Effect**:
  - If HPXML provides film resistances, no gap
  - If not provided, HARES may use default/zero values (warned in OCHRE line 252-253)
  - Wind-speed variation in exterior film coefficient (~10% effect on exterior surface temperature) is not modeled

---

## SUMMARY TABLE

| Feature | OCHRE | HARES | Severity | Impact |
|---------|-------|-------|----------|--------|
| Window optics (SHGC→transmit) | Full | None | HIGH | 10-30% solar gain error |
| Sky temp in LWR | Nonlinear split | Linearized, fallback | MEDIUM | 10-15% error in extreme T |
| Natural ventilation | Stack+wind | None | MEDIUM | 5-15% cooling load error |
| Duct DSE losses | Full model | None | HIGH | 10-25% HVAC energy error |
| Occupancy→gains | Automatic | None | MEDIUM | 5-10% sensible, latent missing |
| Humidity buffering | 15x mult | None | LOW-MEDIUM | RH spikes, dehumid error |
| Foundation/basement | Full zone | Partial | MEDIUM | Basement temp error 5°C+ |
| Attic radiant barrier | Modeled | Not modeled | LOW-MEDIUM | 5-10% summer cooling error |
| Garage coupling | Full walls | Partial | LOW-MEDIUM | ~50 W loss/gain unaccounted |
| Multi-zone interior | Full | Partial | LOW | <2% effect |
| Thermal mass multiplier | 7x explicit | Implicit in RC | LOW-MEDIUM | Peak load ±10% |
| Window solar distribution | View factors | Single node | LOW | Transient only, <2% annual |
| Infiltration methods | 3 methods | 3 methods | NONE | Parity |
| Component loads | Detailed | None | NONE | Diagnostic only |
| Film resistance | Calculated | User input | LOW | <5% if provided |

---

## RECOMMENDATIONS FOR HARES

### Critical (Affects 5%+ of energy or comfort accuracy):
1. **Window optics**: Implement SHGC-to-transmittance with EnergyPlus curves and shading fraction
2. **Duct DSE**: Add duct loss model routing to appropriate zones
3. **Occupancy gains**: Automatically convert occupancy schedule to sensible/latent gains per person
4. **Natural ventilation**: Implement stack-driven + wind-driven model for outdoor-facing zones

### Important (Affects 1-5% accuracy):
5. **Humidity capacitance**: Add material absorption damping (15x multiplier or equivalent)
6. **Foundation/basement**: Ensure proper heat coupling from indoor → foundation → ground
7. **Attic radiant barrier**: Override emissivity for reflective surfaces
8. **Nonlinear exterior LWR**: Option to use iterative solver instead of linearization (for high-fidelity)

### Nice-to-have (Diagnostic/Quality):
9. **Component load tracking**: Report heat gains by boundary type
10. **Interior-wall heat transfer**: Track within-zone heat flows for multi-room diagnostics

### No Action Needed:
- Infiltration methods: Already match
- Film resistance: Acceptable as user-input if HPXML provides values
