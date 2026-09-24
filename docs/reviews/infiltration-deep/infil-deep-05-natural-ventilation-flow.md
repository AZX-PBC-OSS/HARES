# Natural ventilation flow formula — wind and stack sign conventions, ASHRAE HOF verification
**Review ID**: infil-deep-05
**Category**: infiltration-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/infiltration.rs` (lines 239–278)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/ZoneEquipmentManager.cc` (lines 5942–6033 — `ZoneVentilation:WindandStackOpenArea` runtime calculation)
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceAirManager.cc` (lines 1965–2110 — `ZoneVentilation:WindandStackOpenArea` input parsing)
- `vendors/EnergyPlus/src/EnergyPlus/DataHeatBalance.hh` (lines 1141–1188 — `VentilationData` struct with `OpenArea`, `openAreaFracSched`, `OpenEff`, `EffAngle`, `DH`, `DiscCoef`)
- `crates/hares-physics/src/infiltration.rs` (lines 130–175 — `ashrae_wind_stack` and `ela_infiltration` companion functions; lines 627–670 — `calculate_ela_coefficients`)

## Findings

### Finding 1: Missing wind pressure coefficient (Cp) for opening orientation — severity: high
**Description**: HARES applies a fixed effectiveness factor of 0.6 for all natural ventilation openings, regardless of opening orientation relative to the wind direction. The ASHRAE Handbook of Fundamentals (2009 Ch. 16.14, Equation 37) and EnergyPlus both specify that `Cw` (opening effectiveness) must vary with the angle between the wind direction and the opening normal:
- Perpendicular winds (wind directly toward the opening): Cw = 0.5–0.6
- Diagonal winds (45° from normal): Cw = 0.25–0.35
- Any angle > 90° (wind on the opposite side): Cw = 0.0 (no effective flow)

EnergyPlus computes this automatically (ZoneEquipmentManager.cc:5988–6004) by calculating the absolute angle between wind direction and `EffAngle`, then linearly interpolating Cw between 0.55 at 0° and 0.3 at 45°, dropping to 0.0 at > 90°.

HARES uses a constant 0.6 for all conditions (line 263: `let nat_vent_area_cm2 = open_area_m2 * 0.6 * M2_TO_CM2`), which:
- Overestimates flow by ~2× when wind is diagonal to the opening (0.6 vs 0.3)
- Produces non-zero flow when wind is on the opposite side of the opening (should be zero)
- Provides no mechanism for the user to specify opening orientation

**Code Location**: `crates/hares-physics/src/infiltration.rs:263`
**Root Cause**: The function is designed to match OCHRE's simplified natural ventilation model, which does not model wind direction. The `open_area_m2` parameter carries no orientation information, and no `EffAngle` equivalent exists in the HARES type system.
**Impact**: For buildings with windows on multiple orientations, natural ventilation flow is incorrectly estimated. Wind blowing against the leeward side of a building would still produce simulated ventilation through leeward openings, which should actually produce zero flow. Diagonal winds produce flow ~100% higher than ASHRAE HOF predicts, leading to overestimated natural cooling benefits.

### Finding 2: No neutral pressure level (NPL) distinction for single-sided vs. cross-ventilation — severity: medium
**Description**: The ASHRAE Handbook of Fundamentals treats stack-driven natural ventilation differently for single-sided and cross-ventilation configurations:
- **Cross-ventilation**: Neutral pressure level (NPL) ≈ mid-height of the opening; ΔP_stack = ρ·g·ΔH·|ΔT|/T_avg, where ΔH is the vertical distance between the inlet (lower) and outlet (upper) openings.
- **Single-sided ventilation**: NPL varies significantly and is less than mid-height; flow is driven by a combination of mean buoyancy and fluctuating (turbulent) effects with a Cd ≈ 0.15–0.25 rather than the full CDC 0.6–0.65.

EnergyPlus uses an explicit `DH` (height difference) parameter for the stack term: `Qst = Cd × Area × √(2·g·DH·|ΔT|/T_zone)` (ZoneEquipmentManager.cc:6013–6014). The user specifies `DH` individually per opening, which allows modeling the vertical separation between openings.

HARES repurposes the infiltration ELA stack coefficient (computed in `calculate_ela_coefficients`, lines 627–670), which assumes a neutral pressure level fraction `NL = 0.5` (line 640). This NPL value is appropriate for distributed envelope leakage but not for discrete intentional openings.

**Code Location**: `crates/hares-physics/src/infiltration.rs:640` (NL = 0.5 in `calculate_ela_coefficients`), reused as `stack_coeff` parameter at line 270.
**Root Cause**: The `natural_ventilation_flow_m3_s` function shares the same `stack_coeff`/`wind_coeff` interface as `ela_infiltration` (lines 233–234: "same as infiltration ELA coeff"), but the physics of intentional openings differ from distributed leakage — specifically in NPL and height-difference effects.
**Impact**: Stack-driven ventilation for intentional openings may be misestimated because the NPL = 0.5 assumption does not distinguish between single-sided (NPL < 0.5) and cross-ventilation (NPL ≈ 0.5, but with explicit height-difference effects). The absence of an explicit opening height difference (`DH`) parameter prevents modeling of vertically separated openings that maximize stack-driven flow.

### Finding 3: Combined wind-and-stack flow — sign convention and quadrature verification — severity: low
**Description**: Both HARES and EnergyPlus assume wind and stack pressures always reinforce (add constructively). This was verified and is consistent with ASHRAE HOF simplified natural ventilation models for single-zone analysis.

HARES computes (line 270):
```
driver = stack_coeff * |ΔT| + wind_coeff * v²
Q = K * sqrt(driver)
```
This is algebraically equivalent to quadrature combination: `Q = sqrt(Qs² + Qw²)` where `Qs = K*sqrt(Cs*|ΔT|)` and `Qw = K*sqrt(Cw)*v`. EnergyPlus uses the same approach explicitly (ZoneEquipmentManager.cc:6015): `VVF = sqrt(Qw² + Qst²)`.

Both models use absolute temperature difference (`|ΔT|`) and squared wind speed (`v²`), which means both always produce positive stack and positive wind contributions. Neither model accounts for:
- Wind opposing stack flow (reducing net ventilation, possible in real buildings)
- Negative stack pressure when indoor is cooler than outdoor (winter infiltration vs. summer ventilation)

However, HARES gates the function to only operate when `t_zone > t_outdoor` (line 253: `t_zone_c <= t_outdoor_c → return 0.0`), so the winter infiltration case where stack and wind might oppose is explicitly excluded for natural ventilation. This gating is correct for the intended "cooling-mode operable window" use case.

**Code Location**: `crates/hares-physics/src/infiltration.rs:270–273`
**Root Cause**: Intentional design choice (matching OCHRE). Not a bug — documented correctly.
**Impact**: No material error for the intended use case (summertime cooling via operable windows). The quadrature combination of wind and stack terms is mathematically consistent with ASHRAE HOF and EnergyPlus.

### Finding 4: No opening area scheduling — fully closed openings correctly produce zero flow, but no timestep-level modulation — severity: medium
**Description**: EnergyPlus provides an `openAreaFracSched` (opening area fraction schedule) parameter that modulates the effective opening area at each timestep (ZoneEquipmentManager.cc:6012–6013). This allows modeling occupant behavior (windows closed at night, open during the day) and control strategies.

The HARES function correctly returns zero flow when `open_area_m2 <= 0.0` (line 255): `|| open_area_m2 <= 0.0 → return 0.0`. For non-zero opening areas, the function has no schedule mechanism — it computes flow from the fixed `open_area_m2` parameter. The caller (thermal_solver/infiltration.rs:166) passes the static `nv.open_area_m2` from configuration, which is computed once at startup as `total_window_area_m2 * 0.067` (config.rs:179).

There is no way to model:
- Window opening fraction varying by time of day
- Temperature-driven window opening/closing strategies
- Occupant stochastic behavior for window operation

**Code Location**: `crates/hares-physics/src/infiltration.rs:255` (zero-flow gate); `crates/hares-envelope/src/thermal_solver/config.rs:179` (static open area)
**Root Cause**: The OCHRE model this is based on does not include an opening fraction schedule. The `NaturalVentilationConfig` struct (config.rs:176–186) lacks a schedule field.
**Impact**: All operable windows are modeled as either fully open (at the configured fraction) or fully closed (when gating conditions fail). There is no ability to simulate partial opening, time-scheduled operation, or temperature-controlled natural ventilation strategies. This may overestimate natural ventilation cooling benefits during hours when occupants would realistically keep windows closed.

### Finding 5: Discharge coefficient (Cd) — fixed vs. temperature-dependent and opening-type-dependent — severity: low
**Description**: ASHRAE HOF and EnergyPlus distinguish between the opening effectiveness `Cw` (wind-side) and the discharge coefficient `Cd` (stack-side). In EnergyPlus (ZoneEquipmentManager.cc:6007–6010):
- `Cd` defaults to `0.40 + 0.0045 × |ΔT|` when auto-calculated, varying from ~0.40 (isothermal) to ~0.49 (at 20 K ΔT)
- `Cd` can be user-specified per opening type (casement: ~0.65, sliding: ~0.45, trickle vents: ~0.15–0.25)

HARES applies a single factor of 0.6 to the opening area (line 263), conflating Cw and Cd into one "effectiveness factor." The value 0.6 most closely matches EnergyPlus's `Cw` for perpendicular winds (0.5–0.6), not EnergyPlus's `Cd` (0.40–0.49 auto-calculated). The HARES documentation at line 262 states "Effectiveness factor 0.6 per EnergyPlus/OCHRE", which is accurate for Cw but does not distinguish Cd for stack-driven flow.

**Code Location**: `crates/hares-physics/src/infiltration.rs:262–263`
**Root Cause**: OCHRE uses the same single-effectiveness-factor approach. HARES faithfully reproduces it. EnergyPlus uses separate Cw and Cd parameters because the ZoneVentilation object was designed for a broader range of opening types.
**Impact**: Minor for typical residential casement windows (Cd ≈ 0.6 is close to EnergyPlus's Cw of 0.5–0.6 for perpendicular winds). Larger discrepancy for sliding windows (Cd should be ~0.45) and trickle vents (Cd should be ~0.2). The model cannot distinguish between different opening types, which limits its accuracy for non-casement windows and dedicated vent openings.

## Summary
- Total findings: 5
- Critical: 0
- High: 1 (Finding 1 — missing wind direction/orientation Cp)
- Medium: 2 (Finding 2 — no NPL distinction; Finding 4 — no opening schedule)
- Low: 2 (Finding 3 — sign convention verified correct; Finding 5 — Cd default acceptable for casement windows)

## Recommendations
1. **Add wind-direction-dependent Cw (priority: high)**: The `natural_ventilation_flow_m3_s` function should accept an opening orientation parameter (azimuth) and the outdoor wind direction. The opening effectiveness should be computed as EnergyPlus does: linear interpolation between 0.55 (perpendicular) and 0.3 (diagonal at 45°), dropping to 0.0 for wind on the opposite side (> 90°). Alternatively, if per-opening orientation is not available, consider using an average Cw ≈ 0.35 that reflects typical wind-direction variability.

2. **Add explicit opening height difference (priority: medium)**: Add a `DH` parameter for stack-driven flow through the opening, replacing the repurposed ELA stack coefficient for the stack term. For single-sided openings, use a reduced Cd (0.15–0.25) to account for the lower driving pressure. For cross-ventilation with vertically separated openings, use Cd ≈ 0.6 and apply the full `sqrt(2·g·DH·|ΔT|/T_avg)` formula.

3. **Add opening fraction schedule (priority: medium)**: Add an optional schedule to `NaturalVentilationConfig` that modulates `open_area_m2` at each timestep. Default should be constant 1.0 (fully open when conditions allow) to maintain backward compatibility.

4. **Add per-opening-type Cd parameter (priority: low)**: Allow the user to specify a discharge coefficient Cd (or select from preset opening types: casement, sliding, trickle vent) instead of using a hard-coded 0.6. For alignment with EnergyPlus, separate `Cd` (stack discharge coefficient, default 0.40 + 0.0045·|ΔT|) from `Cw` (wind effectiveness, default auto-calculated from wind angle).

## References / Citations
- ASHRAE Handbook of Fundamentals 2009, Chapter 16 (Ventilation and Infiltration), Equation 37: `Q = Cw × A × U` for wind-driven natural ventilation
- ASHRAE Handbook of Fundamentals 2017/2021, Chapter 16 (Ventilation and Infiltration)
- EnergyPlus Engineering Reference §15.4, ZoneVentilation:WindandStackOpenArea: `Q = √(Qw² + Qst²)` where `Qw = Cw × A × f_sched × U` and `Qst = Cd × A × f_sched × √(2·g·ΔH·|ΔT|/T_zone)`
- EnergyPlus `ZoneEquipmentManager.cc:5942–6033` — `WindAndStack` runtime calculation
- EnergyPlus `DataHeatBalance.hh:1180–1187` — `WindandStackOpenArea` struct fields (OpenArea, openAreaFracSched, OpenEff, EffAngle, DH, DiscCoef)
- OCHRE `envelope.py:_natural_ventilation` — source of the HARES simplified model
- Walker & Wilson (1998) "Field Validation of Algebraic Equations for Stack and Wind Driven Air Infiltration Calculations", *HVAC&R Research* 4(2):139–163 — NPL = 0.5 assumption for distributed leakage
