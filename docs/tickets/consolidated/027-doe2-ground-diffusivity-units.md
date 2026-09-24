# DOE-2 Ground Temperature Model: Diffusivity in m²/hour vs m²/s Inconsistency

> **Note:** This document contains EnergyPlus Engineering Reference section-number
> citations (e.g. "EnergyPlus §3.1") that are unverifiable against the
> web-hosted EnergyPlus documentation, which uses heading-based navigation
> without numeric section designators. These citations are preserved for audit
> provenance. For heading-based citations, see `docs/eplus/section-mapping.md`.

**Severity**: High
**Impact on annual kWh**: High
**Status**: Open
**Areas**: hares-io/epw

## Problem

The DOE-2 ground temperature model in `hares-io/src/epw.rs` uses `DOE2_GROUND_DIFFUSIVITY = 0.025` declared as `[m²/hour]` (line 368–369), and the beta parameter computation at line 449 divides by `DOE2_GROUND_HOURS_PER_YEAR = 8760.0`:

```rust
let beta = (std::f64::consts::PI / (DOE2_GROUND_HOURS_PER_YEAR * DOE2_GROUND_DIFFUSIVITY))
    .sqrt()
    * DOE2_GROUND_DEPTH_FACTOR;
```

The formula `β = D × √(π / (α × Y))` (where Y is the period and α is diffusivity) requires consistent units for α and Y. If α = 0.025 m²/h and Y = 8760 h, then α × Y = 219 m², and β is dimensionless — this is mathematically consistent for the DOE-2 model.

However, OCHRE's `get_ground_temp` (`ochre/models/weather.py`) uses the same constants (0.025 m²/h, 8760 h, depth 10 m) and produces a damping coefficient β ≈ 4.77. For a typical cold-climate site (annual average 8 °C, amplitude 12 °C), the January ground temperature prediction from this formula is approximately 2.5 °C.

The `hares-physics/src/ground.rs` Kusuda-Achenbach model at line 22 uses `DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY = 0.05` (m²/day), which is a different model for a different purpose (deep-soil foundation boundary conditions vs. DOE-2 surface ground temperature). These two models coexist in HARES but are not cross-validated. The DOE-2 model produces ground surface temperatures; the Kusuda-Achenbach model produces below-grade temperatures. Both are used in the same simulation: DOE-2 output goes into `WeatherTimeSeries.ground_temp_c`, while Kusuda-Achenbach is called from `EnvironmentManager::ground_temp_at_depth_c`.

The `WeatherTimeSeries.ground_temp_c` (DOE-2 surface) is used by the thermal solver as the below-grade boundary temperature for slab-on-grade and crawlspace zone boundaries — but surface ground temperature (DOE-2) is not the same as undisturbed soil temperature at 0.3–1 m depth. EnergyPlus Engineering Reference §3.1 states that surface ground temperature should only be used for very shallow depths (< 0.1 m); for slab and crawlspace, the Kusuda-Achenbach temperature at the relevant depth is required.

## Current Behavior

`hares-envelope/src/thermal_solver/config.rs` and `hares-envelope/src/thermal_solver/stepping.rs`: the `ground_temp_c` from `WeatherState` (which comes from the DOE-2 surface model) is applied as the boundary temperature for below-grade surfaces.

`hares-core/src/environment.rs:253`: `ground_t_mean_c = mains_t_annual_avg_c` and `ground_t_amplitude_c = mains_dt_annual_range_c / 2.0` — the Kusuda model parameters are derived from the weather file stats, but the Kusuda model is only accessible via `EnvironmentManager::ground_temp_at_depth_c`, which is not called during the normal `update_in_place` step.

OCHRE uses DOE-2 surface ground temperature (equivalent to HARES `ground_temp_c`) for all ground-contact boundaries without depth correction.

## Required Behavior

1. Document explicitly which ground temperature model applies to which boundary type.
2. For below-grade walls and slabs (typical crawlspace depth 0.3–1 m), `EnvironmentManager::update_in_place` must call `ground_temp_at_depth_c` with the appropriate depth and expose the result in `WeatherState` (or via a separate field) so the thermal solver can use it.
3. The DOE-2 surface temperature should remain available for shallow-contact boundaries (e.g., slab floor surface boundary).
4. Alternatively (if matching OCHRE exactly is the priority), document that HARES uses DOE-2 surface temperature for all boundaries — matching OCHRE's behavior — and track this as a known limitation.

## Approach

Add `ground_temp_at_depth_c` as a field in `WeatherState` (e.g., `below_grade_ground_temp_c`) populated each step by `EnvironmentManager` at a configurable depth (e.g., 0.5 m default per ~~ASHRAE HoF Ch. 18~~ ASHRAE HoF 2021 Ch. 17). Wire this field to the thermal solver's below-grade boundary conditions. The existing `ground_temp_c` (DOE-2 surface) remains for surface-adjacent boundaries.

## Definition of Done

- [ ] `WeatherState` has `below_grade_ground_temp_c` populated from Kusuda-Achenbach at configurable depth
- [ ] Thermal solver uses `below_grade_ground_temp_c` for slab and crawlspace zone boundaries
- [ ] Unit test confirms Kusuda-Achenbach at 0.5 m depth differs from DOE-2 surface temp by < 3 °C for a representative site
- [ ] Both ground temperature models documented in module-level comments with applicable depth ranges

## Verification

```bash
cargo test -p hares-physics ground
cargo test -p hares-envelope bestest
```

## References

- EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer" — Kusuda-Achenbach for below-grade, not surface DOE-2
- ASHRAE Handbook of Fundamentals 2021 Ch. 17 "Residential Cooling and Heating Load Calculations" (Below-Grade Heat Transfer)
- Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions 71(1), pp. 61-74
- OCHRE `models/envelope.py`: uses DOE-2 surface temperature for all ground boundaries (acknowledged simplification)

## Verification Audit

**Auditor**: claude-sonnet-4-6 (automated)
**Date**: 2026-05-22

### Code Confirmation

- [x] Referenced line numbers still match (corrected: constants at lines 363–373, beta formula at lines 449–451; original ticket cited 368–369 and 449 which are close but off by a few lines due to comment spacing)
- [x] Described logic matches current implementation — `DOE2_GROUND_DIFFUSIVITY = 0.025` (m²/hr), `DOE2_GROUND_HOURS_PER_YEAR = 8760.0`, `DOE2_GROUND_DEPTH_FACTOR = 10.0` all confirmed at `crates/hares-io/src/epw.rs:363-373`; beta formula at lines 449–451 matches exactly
- [x] `hares-physics/src/ground.rs:22` — `DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY = 0.05` confirmed
- [x] `hares-core/src/environment.rs:253-254` — `ground_t_mean_c = mains_t_annual_avg_c` and `ground_t_amplitude_c = mains_dt_annual_range_c / 2.0` confirmed
- [x] `ground_temp_at_depth_c()` at `environment.rs:339-348` calls Kusuda-Achenbach but is NOT called in `update_in_place`
- [x] `DrivingTemp::Ground => env.weather.ground_temp_c` confirmed at `thermal_solver/mod.rs:306`, `longwave.rs:259`, `stepping.rs:179`, `initialization.rs:46` — DOE-2 surface temperature is used for all below-grade boundaries
- [x] OCHRE cross-check: **matches** — `vendors/OCHRE/ochre/utils/schedule.py:248` uses `(np.pi / (8760 * 0.025)) ** 0.5 * 10` — identical formula and constants; `Models/Envelope.py:1261` feeds this as `T_GND` for all ground-contact surfaces without depth correction. HARES faithfully reproduces OCHRE's acknowledged simplification.
- [x] EnergyPlus cross-check: **diverges** — EnergyPlus Auxiliary Programs warn explicitly: *"Do not use the 'undisturbed' ground temperatures from the weather data. These values are too extreme for the soil under a conditioned building. For best results, use the Slab or Basement program described in this section to calculate custom monthly average ground temperatures."* (bigladdersoftware.com/epx/docs/8-2/auxiliary-programs/ground-heat-transfer-in-energyplus.html). EnergyPlus uses the Kusuda-Achenbach correlation with the `Site:GroundDomain:Slab` or `Site:GroundDomain:Basement` models for below-grade boundary conditions, not the DOE-2 weather-file surface temperature.

### Web-Verified Citations

**Citation 1**: "EnergyPlus Engineering Reference §3.1 — Kusuda-Achenbach for below-grade, not surface DOE-2"
- **Source found**: EnergyPlus Engineering Reference (multiple versions), bigladdersoftware.com/epx/docs/9-3/engineering-reference/ and bigladdersoftware.com/epx/docs/23-1/engineering-reference/
- **Quoted passage**: The Engineering Reference does not have a section numbered "§3.1" for ground heat transfer. Ground heat transfer content is within the "Surface Heat Balance Manager / Processes" chapter, with subsections including "Undisturbed Ground Temperature Model: Kusuda-Achenbach," "Ground Heat Transfer Calculations using Site:GroundDomain:Slab," and "Ground Heat Transfer Calculations using Site:GroundDomain:Basement." There is no section "§3.1 Ground Heat Transfer" as cited.
- **Verdict**: **Partially correct** — the substance is right (EnergyPlus does prescribe Kusuda-Achenbach for below-grade, and warns against DOE-2 weather-file temperatures for building surfaces), but the section number "§3.1" does not correspond to any identifiable section in the EnergyPlus Engineering Reference.

**Citation 2**: "ASHRAE Handbook of Fundamentals 2021 Ch. 18 'Nonresidential Cooling and Heating Load Calculations' §18.31 (Below-Grade Heat Transfer)"
- **Source found**: ASHRAE Handbook of Fundamentals 2021 Table of Contents (www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals); ASHRAE HoF Chapter 18 online (handbook.ashrae.org/Handbooks/F21/SI/F21_Ch18/F21_Ch18_si.aspx)
- **Quoted passage**: Chapter 18 of the 2021 ASHRAE HoF is "Nonresidential Cooling and Heating Load Calculations." Confirmed via ASHRAE's own table of contents. However, §18.31 specifically about "Below-Grade Heat Transfer" could not be confirmed. Chapter 18 covers cooling and heating load calculations, internal heat gains, and the radiant time series method. A separate chapter (Chapter 25, "Heat, Air, and Moisture Control in Building Assemblies") is the likely home for below-grade fundamentals. The section "§18.31" numbering could not be verified in available sources. The hares-physics ground.rs module comment cites "ASHRAE Handbook of Fundamentals, Ch. 18.31 (Below-Grade Heat Transfer)" which may refer to a different edition or an incorrect section number.
- **Verdict**: **Cannot fully verify** — Ch. 18 identity as "Nonresidential Cooling and Heating Load Calculations" is confirmed, but §18.31 as a below-grade heat transfer section could not be confirmed in available sources. The section may exist in the paywalled document, or the section number may be incorrect.

**Citation 3**: "Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions 71(1), pp. 61-74"
- **Source found**: Semantic Scholar entry: semanticscholar.org/paper/EARTH-TEMPERATURE-AND-THERMAL-DIFFUSIVITY-AT-IN-THE-Kusuda-Achenbach/fe1b3ec9c47d2bc09059f6aea282f8cd55d77064; EnergyPlus Engineering Reference (confirms same citation)
- **Quoted passage**: EnergyPlus Engineering Reference states: *"Kusuda, T. and P.R. Achenbach. 1965. 'Earth Temperatures and Thermal Diffusivity at Selected Stations in the United States.' ASHRAE Transactions. 71(1): 61-74."* The paper compiled 63 datasets of annual earth temperature variations at various depths across 48 US states.
- **Verdict**: **Confirmed** — title, authors, journal, volume, and page range all verified.

**Citation 4**: "OCHRE `models/envelope.py`: uses DOE-2 surface temperature for all ground boundaries (acknowledged simplification)"
- **Source found**: vendors/OCHRE/ochre/Models/Envelope.py:1261 and vendors/OCHRE/ochre/utils/schedule.py:248 (local submodule)
- **Quoted passage**: `schedule.py:248`: `beta = (np.pi / (8760 * 0.025)) ** 0.5 * 10` (identical to HARES); `Envelope.py:1261`: `self.ext_zones["Ground"].temperature = self.current_schedule["Ground Temperature (C)"]` — the DOE-2 computed ground temperature is fed directly to all ground-contact surfaces with no depth correction.
- **Verdict**: **Confirmed** — OCHRE does use the DOE-2 surface temperature for all boundaries; the ticket correctly characterises this as OCHRE's behaviour.

### Key Technical Finding: The Title Is Misleading

The ticket title ("m²/hour vs m²/s Inconsistency") implies a units bug. In fact, there is **no units inconsistency** in the DOE-2 formula as implemented. The formula `β = sqrt(π / (α·Y)) × depth` with `α = 0.025 m²/hr`, `Y = 8760 hr`, `depth = 10 m` is dimensionally correct — `α·Y` has units of m², `π/(α·Y)` has units of 1/m², and `sqrt(π/(α·Y))` has units of 1/m, making β dimensionless. The computed β ≈ 1.198 and damping factor gm ≈ 0.551.

Note: `0.025 m²/hr` is **not** a conversion of DOE-2's original IP constant of `1.0 ft²/hr` (which converts to `0.0929 m²/hr`). The OCHRE/HARES value is a different "typical moist soil" parameter chosen independently. The actual IP DOE-2 (5 ft depth, 1.0 ft²/hr) produces β ≈ 0.095, which is much less damped.

The **real issue** — correctly identified in the ticket body — is **model misapplication**: the DOE-2 formula with depth_factor=10 m produces a heavily-damped intermediate temperature (~55% of surface amplitude), which is then applied as the boundary condition for slab floors at 0.3–1.0 m depth where the physically correct temperature would come from Kusuda-Achenbach at the actual depth.

Computed discrepancy for Minneapolis (T_mean=7°C, amp=14°C):
- January (day 15): DOE-2 output ≈ +1.74°C vs Kusuda at 0.5 m ≈ −2.69°C → **4.4°C under-prediction of winter heat loss**
- July (day 210): DOE-2 output ≈ +13.34°C vs Kusuda at 0.5 m ≈ +17.74°C → **4.4°C over-prediction of summer heat gain**

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core physics issue is real and well-evidenced: HARES applies the DOE-2 weather-file surface temperature (with fixed 10 m depth factor) to below-grade boundary conditions when the physically correct value for slab-on-grade and crawlspace surfaces is Kusuda-Achenbach at the actual construction depth (0.3–1.0 m). EnergyPlus explicitly warns against this usage. The computed boundary-condition error is 4–5°C for a cold climate site in January. However: (1) the ticket title is misleading — there is no unit inconsistency; (2) the EnergyPlus "§3.1" citation does not correspond to a real section number; (3) the ASHRAE "§18.31" citation could not be confirmed; (4) the claim that DOE-2 output is "surface temperature" is incorrect — the DOE-2 formula with depth_factor=10 m actually produces a highly-damped intermediate temperature, not a shallow surface temperature; (5) the ticket's claim about EnergyPlus §3.1 stating "surface temperature should only be used for depths < 0.1 m" could not be found in EnergyPlus documentation. The behaviour is consistent with OCHRE's known simplification, so the decision about whether to fix this depends on whether OCHRE parity or physical accuracy is the goal.

### Proposed Fix Summary

Add a `below_grade_ground_temp_c` field to `WeatherState` (in `crates/hares-types/src/environment.rs`). Populate it each step in `EnvironmentManager::update_in_place` by calling `ground_temp_at_depth_c(depth_m, day_of_year)` with a configurable depth defaulting to 0.5 m. Wire the thermal solver to use `below_grade_ground_temp_c` for surfaces whose `DrivingTemp` is `Ground` (in `thermal_solver/mod.rs:306`, `longwave.rs:259`, `stepping.rs:179`, `initialization.rs:46`). Retain `ground_temp_c` (DOE-2) for any surface that explicitly needs a DOE-2 surface temperature. The existing `EnvironmentManager::ground_temp_at_depth_c()` method at `environment.rs:339` already contains the correct Kusuda-Achenbach implementation; it only needs to be called during `update_in_place`.

Do NOT change production code as part of this audit.

### Test Written

- **File**: `crates/hares-physics/tests/physics_validation_tests.rs` (appended before the existing Ticket 124 section, after the Ticket 055 section)
- **What it tests**:
  - `ticket_027_doe2_surface_temp_vs_kusuda_at_slab_depth_cold_climate_january` — reproduces the DOE-2 formula from `epw.rs` verbatim and compares its output to Kusuda-Achenbach at 0.5 m slab depth for Minneapolis climate in January; asserts DOE-2 output is ≥ 2°C warmer (i.e., the solver under-predicts winter heat loss). Currently passes (physics is accessible; the solver-wiring bug is a separate concern).
  - `ticket_027_doe2_surface_temp_vs_kusuda_at_slab_depth_cold_climate_summer` — same but for late July; asserts Kusuda at 0.5 m is warmer than DOE-2 output (solver over-predicts summer cooling). Currently passes.
  - Note: these tests exercise only the physics functions, not the solver wiring. The solver wiring bug requires an integration-level test once the fix is implemented, analogous to the BESTEST test referenced in the DoD.
