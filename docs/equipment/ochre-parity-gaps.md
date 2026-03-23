# HARES vs OCHRE: Parity Gap Analysis

[Back to Architecture](../architecture.md)

This document catalogs where HARES is **behind OCHRE** in capabilities, accuracy, or flexibility. Items are ranked by impact on simulation fidelity for residential energy modeling. Areas where HARES exceeds OCHRE are noted but not the focus.

---

## Critical Gaps (Block accurate results for common scenarios)

### 1. Interior Longwave Radiation — Linearized vs Iterative

**OCHRE**: Iterative non-linear T^4 solver with heavy-ball damping (0.3x new + 0.2x momentum), convergence to 0.01C, per-surface radiation temperatures updated each iteration.

**HARES**: Linearized `h_r = 4*epsilon*sigma*T_avg^3` around zone air temperature. No iterative refinement.

**Impact**: Underestimates interior radiation accuracy during transient/peak conditions. Error grows with temperature swings (e.g., passive solar homes, unconditioned spaces).

### 2. Window Solar Decomposition — Missing Transmittance Curves

**OCHRE**: 6 polynomial transmittance curve families (A, B/D, D, E, F, J) selected by U-factor and SHGC per EnergyPlus Engineering Reference. Separate absorptivity calculation. 0.854x diffuse fudge factor.

**HARES**: Simple SHGC-based split: `absorbed_inward = max(0, SHGC - transmittance)`. No angle-dependent polynomial curves. No diffuse correction factor.

**Impact**: Systematic error in transmitted solar — overestimates on high-SHGC windows, misses beam attenuation at steep angles.

### 3. Solar Distribution to Interior Surfaces

**OCHRE**: Routes transmitted solar to all interior surfaces proportionally via window view factors.

**HARES**: Transmitted solar goes directly to zone air node, not distributed to surfaces.

**Impact**: Interior surface temperatures are underestimated, affecting interior radiation balance and comfort predictions.

### 4. Ground Coupling / Foundation Modeling

**OCHRE**: RC-based slab modeling with HPXML perimeter/underslab insulation parsing (`get_slab_insulation()`, `get_fnd_wall_insulation()`).

**HARES**: Fixed ground temperature boundary condition. No depth-aware ground contact, no perimeter insulation modeling.

**Impact**: Cannot accurately model slab-on-grade or basement thermal performance — significant for heating-dominated climates.

### 5. Battery Temperature-Dependent Capacity Derating

**OCHRE**: 2-term Arrhenius-exponential model reduces available capacity at extreme temperatures each timestep.

**HARES**: Power limits are static regardless of cell temperature. Lumped thermal model exists but doesn't feed back into capacity.

**Impact**: Overestimates cold-weather battery performance by 20-30%, underestimates hot-weather degradation risk.

### 6. V2G / V2H — Explicitly Blocked

**OCHRE**: Full bidirectional support with import/export limits.

**HARES**: `HaresError::Control("V2G not supported in v1")` — explicitly blocked for both battery and EV.

**Impact**: Cannot model grid-support, demand response via discharge, or arbitrage scenarios.

---

## High Gaps (Affect accuracy for specific but common scenarios)

### 7. Boiler EIR Curves

**OCHRE**: 6-coefficient condensing curve, 10-coefficient non-condensing curve with PLR and water temperature dependence.

**HARES**: Constant efficiency model only. No part-load or temperature-dependent efficiency.

**Impact**: Significant for hydronic heating systems — boiler efficiency varies 10-20% across operating range.

### 8. Ventilation Fan / HRV / ERV Equipment

**OCHRE**: Ventilation fan modeled as ScheduledLoad with configurable schedules.

**HARES**: No ventilation equipment type exists in the codebase.

**Impact**: Cannot model mechanical ventilation energy use or heat recovery.

### 9. HPWH Wet-Bulb Temperature Input

**OCHRE**: COP/capacity biquadratic curves are f(T_wet_bulb, T_tank).

**HARES**: Uses dry-bulb ambient temperature instead of wet-bulb for HPWH curves.

**Impact**: COP values ~10-15% too optimistic in humid climates, pessimistic in dry climates.

### 10. PV Shading / Building Integration

**OCHRE**: SAM/PVWatts v8 integration with irradiance decomposition (DNI/DHI/GHI tracking) and building shading.

**HARES**: Pre-computed LUT or simple PVWatts model. No building feature shading.

**Impact**: Overestimates PV output for partially shaded arrays — common in residential settings.

### 11. Weather Derived Fields

**OCHRE**: Computes sky temperature, mains water temperature (Burch-Christensen), ground temperature (DOE-2 monthly sinusoidal), humidity ratio, wet-bulb from weather data.

**HARES**: Basic EPW parser. Some derived fields computed in EnvironmentManager but mains temp, ground temp may use simpler models.

**Impact**: Accuracy of boundary conditions affects all downstream thermal calculations.

### 12. Output Metrics Completeness

**OCHRE**: 50+ metrics including islanding time, per-equipment COP, duct system efficiency, component loads (infiltration/ventilation/ducts), voltage/frequency metrics.

**HARES**: ~10 metrics: annual energy, peak power, comfort hours, grid interaction.

**Impact**: Cannot validate against EnergyPlus or provide detailed audit-grade output without additional metric computation.

### 13. EV Fleet Modeling

**OCHRE**: PDF-based arrival/departure/SOC distributions from EVI-Pro, per-vehicle archetypes (PHEV20/50, BEV100/250).

**HARES**: Single-vehicle model with 7 driver archetypes. No fleet aggregation or distribution-based sampling.

**Impact**: Cannot model aggregate EV demand for grid studies.

---

## Medium Gaps (Refinements affecting specific conditions)

### 14. Thermal Bridging / Framing Factors

**OCHRE**: Material library has construction-type lookup for framing effects.

**HARES**: Assumes uniform U-value across boundary. No cavity-vs-stud R-value splitting.

### 15. ASHP Backup Heating Control

**OCHRE**: Full 4-mode FSM (HP On, HP+ER, ER Only, Off) with hard/soft lockout timing, setpoint change detection, ER fan power adjustment.

**HARES**: Partial backup heating — has backup capacity/EIR fields but missing soft lockout, setpoint change detection, staged backup architecture.

### 16. HPWH HP/ER Independent Duty Cycle Control

**OCHRE**: Separate `"HP Duty Cycle"` and `"ER Duty Cycle"` control signals for fine-grained DR.

**HARES**: Generic duty cycle control — cannot independently curtail HP compressor vs backup element.

### 17. Speed Disabling / Override

**OCHRE**: Dynamic `"Disable Speed X"` control signals during operation.

**HARES**: Fixed speed configuration at initialization. No runtime speed disable capability.

### 18. MSHP Cooling Speed Remapping

**OCHRE**: Special 10-to-4 speed remapping for minisplit cooling mode.

**HARES**: Speed remapping implemented for heating only, not cooling.

### 19. Schedule Timezone & Seasonal Support

**OCHRE**: pytz timezone-aware scheduling with DST support, seasonal month multipliers, schedule modification at runtime.

**HARES**: No timezone-aware scheduling, no seasonal multipliers, no runtime schedule modification.

### 20. PLF Disable Option

**OCHRE**: Configurable `"Disable HVAC Part Load Factor"` for testing/calibration.

**HARES**: No option to disable part-load factor — hardcoded in model.

### 21. Battery Degradation Granularity

**OCHRE**: Per-cycle DOD values from rainflow for cycle aging (b2 term uses individual cycle amplitudes).

**HARES**: Accumulated daily sum of DOD^2 — less accurate when duty cycles are highly variable within a day.

### 22. Co-Simulation / HELICS Integration

**OCHRE**: HELICS federate interface for multi-agent grid simulation.

**HARES**: No co-simulation framework. Python API supports external control but not federate coordination.

### 23. Equipment-Level ZIP Reactive Power

**OCHRE**: Per-equipment ZIP reactive power calculation with voltage-dependent mode switching.

**HARES**: ZIP model in scheduled loads only. No base equipment-level ZIP reactive power or voltage-dependent mode control.

### 24. Attic/Crawlspace Parameterization

**OCHRE**: Flexible per-zone infiltration config, vented attic/crawlspace with configurable parameters.

**HARES**: Hardcoded defaults (2.0 ACH vented attic, 1.5m triangular height). Less configurable.

### 25. Validation & Analysis Tools

**OCHRE**: BESTEST integration, EnergyPlus comparison tools, psychrometric validation, analysis/visualization utilities.

**HARES**: Unit tests only. No systematic validation framework, no EnergyPlus comparison, no built-in analysis tools.

---

## Areas Where HARES Exceeds OCHRE

For completeness, these are areas where HARES is already better:

| Area | HARES Advantage |
|------|-----------------|
| **Defrost models** | Both OnDemand + Timed (OCHRE: OnDemand only) |
| **Crankcase heater** | Temperature-dependent capacity curve polynomial (OCHRE: fixed kW) |
| **Latent degradation** | Full Henderson-Rengarajan model (OCHRE: Ao-only) |
| **Generator CHP** | Fully implemented thermal + fluid ports (OCHRE: stubbed) |
| **DHW coupling** | Port-based fluid integration (OCHRE: separate event files) |
| **Demand response framework** | Typed ControlSignal enum with capability gating (OCHRE: dict-based) |
| **Film coefficients** | Configurable surface roughness enum (OCHRE: fixed roughness factor) |
| **Architecture** | Modular Rust crates with strong typing (OCHRE: monolithic Python) |
| **Performance** | 10-50x faster parsing, zero hot-path allocation (OCHRE: Python overhead) |
| **Checkpointing** | Full state serialization via postcard (OCHRE: no checkpoint/restore) |
| **Humidity solver** | Rigorous bisection-based psychrometric solver (OCHRE: psychrolib) |

---

## Priority Ranking for Closing Gaps

Ordered by impact on simulation accuracy for typical residential scenarios:

1. **Interior LWR iterative solver** — affects all buildings, every timestep
2. **Window transmittance curves** — affects all buildings with windows (i.e., all of them)
3. **Solar distribution to surfaces** — affects passive solar, high-glazing buildings
4. **Ground coupling** — affects slab-on-grade and basement buildings
5. **Boiler EIR curves** — affects all hydronic heating systems
6. **HPWH wet-bulb COP** — affects all heat pump water heaters
7. **Battery temperature capacity derating** — affects cold/hot climate battery modeling
8. **V2G/V2H** — blocks grid-support use cases
9. **Ventilation/HRV/ERV equipment** — affects all mechanically ventilated buildings
10. **Output metrics completeness** — blocks validation and audit workflows
