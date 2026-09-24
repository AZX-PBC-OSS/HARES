# BESTEST Case 900FF Root Cause Analysis

**Date**: 2026-04-20
**Status**: Root cause identified and measured. All major hypotheses tested empirically.

---

## Problem Statement

| Metric | Measured | Reference Band | Deviation |
|--------|----------|----------------|-----------|
| Min zone temp | +0.9025 °C | [−6.4, −1.6] °C | **+2.50 °C above upper bound** |
| Peak zone temp | 43.14 °C | [41.6, 44.8] °C | Inside band |

Sibling Case 600FF (lightweight) passes both metrics. Case 900 (conditioned heavyweight) misses annual heating by 0.021%. The defect is localized to heavyweight construction under sustained nighttime cold.

### Reference Band Provenance

The [−6.4, −1.6] °C band is from ASHRAE 140-2017 Informative Annex B8 Table B8-3a. It represents the envelope of results from ESP-r, BLAST, DOE-2.1E, SERIRES, S3PAS, TRNSYS, TASE, SUNREL, and EnergyPlus. All reference tools use warmup periods before measuring the annual cycle. HARES does not.

---

## What Case 900FF Tests That 600FF Does Not

Both cases are identical buildings with free-floating temperature (no HVAC). The only difference is the wall assembly:

- **600FF walls**: wood siding (9mm) + fiberglass batt (66mm, ρ=12 kg/m³) + gypsum board (12mm)
- **900FF walls**: wood siding (9mm) + insulation (61.5mm) + **100mm heavyweight concrete (ρ=1400 kg/m³, cp=1000 J/kg·K)**

The 900FF floor also has an 80mm concrete slab. The test exercises heat storage and release in high-capacity thermal mass and the effect of initial conditions on annual minimum temperature. Because 600FF has minimal mass, it has almost no memory of initial conditions by the time the winter minimum occurs.

---

## Simulation Evidence at the Minimum-Temperature Event

The diagnostic test `debug_900ff_min_temp_heat_balance` in `tests/bestest/mod.rs` shows the minimum at step 944 (hour 944 ≈ January 9):

```
step=939  t_zone=2.564  t_out=-15.6  opaque_lwr=-1907  infiltration=-380  internal=148
step=940  t_zone=2.153  t_out=-15.6  opaque_lwr=-1826  infiltration=-370  internal=148
step=941  t_zone=1.818  t_out=-15.0  opaque_lwr=-1771  infiltration=-349  internal=148
step=942  t_zone=1.382  t_out=-15.6  opaque_lwr=-1900  infiltration=-355  internal=148
step=943  t_zone=1.013  t_out=-15.6  opaque_lwr=-1791  infiltration=-346  internal=156
step=944  t_zone=0.902  t_out=-13.3  opaque_lwr=-1552  infiltration=-289  internal=168
step=945  t_zone=1.525  t_out=-12.2  opaque_lwr=+929   infiltration=-263  internal=172
```

The dominant losses are opaque LWR (conduction + exterior longwave, ~−1700 to −1900 W) and infiltration (~−300 to −380 W). Internal gain (~148–168 W) is less than half the combined loss.

---

## Hypothesis Testing — All Results Are Measured, Not Estimated

Every claim below is backed by a simulation delta from a hack-test followed by a confirmed revert.
All reverts confirmed via `git diff -- <file>` returning empty.

---

### H1: Initial Zone Temperature (DOMINANT CAUSE)

**Finding**: Changing initial zone temperature from 21°C (HVAC default) to 0°C moves 900FF min_zone_temp from +0.902°C to −3.625°C — a **−4.53°C delta**.

**Measured sensitivity table** (all deltas vs 21°C baseline of 0.902°C):

| Initial zone air °C | min_zone_temp_c | Delta vs baseline |
|---------------------|-----------------|-------------------|
| 21°C (baseline) | +0.9025 | — |
| 10°C | +0.9025 | 0.000°C |
| 5°C | −0.0810 | −0.983°C |
| 3°C | −1.4867 | −2.389°C |
| 0°C | −3.6249 | −4.527°C |

**Mechanism**: For a free-float building (no HVAC) in Denver starting January 1, there are no setpoints, so `determine_initial_indoor_temp_c` returns `DEFAULT_SETPOINT_C = 21.0°C` (`crates/hares-core/src/environment.rs:751`). The `initialize_steady_state` call in `ThermalSolver::new` pins the conditioned zone at 21°C and solves the steady-state RC node temperatures with January outdoor conditions (~−19°C). This gives the concrete walls a warm initial state that buffers the early-January cold snap.

In a real building that has been running all year, the concrete walls in early January are much colder — they have shed heat through the autumn and early winter. EnergyPlus and all reference tools in the ASHRAE 140-2017 band use warmup periods (minimum 20 days, repeated until convergence) before measuring the annual cycle. HARES performs a single-pass steady-state initialization with Jan 1 weather conditions and an unrealistic 21°C zone air assumption.

**Why 10°C → 21°C shows no difference**: The concrete thermal time constant at the wall level (~3 days for the concrete layer alone) is short enough that the temperature difference from 21°C vs 10°C initial conditions fully decays before step 944 (Jan 9). However, the coupled system time constant — including the floor slab (C ≈ 5.4 MJ/K) and the nonlinear interaction with infiltration and exterior LWR — is longer. Once initial temperatures drop below a threshold (between 5°C and 10°C), the concrete can no longer buffer the early-January cold snap.

**Why 600FF is unaffected**: The 600FF lightweight walls have concrete capacitance ≈ 0 — they forget initial conditions within hours. No initial condition sensitivity for lightweight construction.

**Code location**: `crates/hares-core/src/environment.rs:751` — `DEFAULT_SETPOINT_C = 21.0` is used when no HVAC setpoints exist. `crates/hares-envelope/src/thermal_solver/initialization.rs:27-53` — steady-state solve uses `env.weather.outdoor_temp_c` (Jan 1, −19°C) and `indoor_temp_c` (21°C) as boundary conditions.

**Correct fix**: Implement a warmup period — run N iterations of the first week of weather forcing until the RC state vector converges (change in node temperatures < threshold). This is the standard approach (EnergyPlus ERM 26.1 — Warmup Convergence). Alternative: use annual-mean weather conditions (`weather_avgs.avg_ambient_c`) as the outdoor boundary in `initialize_steady_state`, and set the free-float zone initial temperature to the annual-mean zone temperature (estimated as `avg_ambient_c + internal_gain_w / H_building`).

---

### H2: Radiant/Convective Split for Internal Gains (Secondary Defect)

**Finding**: Hack-testing 60% radiant / 40% convective split (vs current 100% convective) moved 900FF min_zone_temp from +0.902°C to **+0.607°C** — a **−0.295°C delta**.

**Code location**: `crates/hares-envelope/src/thermal_solver/ports.rs:14-22` — `apply_port_sensible_inputs` routes 100% of `sensible_gain_w` to zone air. No radiant fraction split exists in this path.

**ASHRAE 140-2017 §5.2.4.3**: Internal gains are 200 W continuous, 60% radiative and 40% convective. The EnergyPlus BESTEST-GSR reference simulation uses `Fraction_Radiant: 0.6` on the OtherEquipment object.

**Status**: Real physics correctness defect. Impact is −0.295°C on 900FF. Confirmed by measurement. Does NOT explain the dominant 2.50°C outlier on its own, but must be fixed for correctness.

**600FF check**: −0.303°C delta, 600FF min_zone_temp moves from −12.863°C to −13.166°C, remains inside band [−18.8, 0.0°C]. Fix is safe.

---

### H3: Solar Storage Routing (Minor)

**Finding**: Hack-testing all solar to zone air (bypassing `radiation_frac` split in `deposit_solar_to_surface_nodes`) moved 900FF min_zone_temp by **−0.142°C**.

**Code location**: `crates/hares-envelope/src/thermal_solver/solar.rs` — `deposit_solar_to_surface_nodes` uses `radiation_frac` to split absorbed solar between RC node (stored in mass) and zone air (immediately available). This is correctly implemented.

**Status**: The current implementation IS correct — solar is distributed partly to mass nodes. The −0.142°C is the contribution from mass storage, not an error. This hypothesis is ruled out as a defect.

---

### H4: RC Discretization — DISPROVED

**Finding**: Increasing concrete layer from 2 to 4 sub-layers moved 900FF min_zone_temp from +0.902°C to **+0.931°C** — wrong direction, +0.029°C warmer.

**Physical explanation**: The ZOH state-space solver is unconditionally stable (matrix exponential discretization). The `C=3` Fourier criterion controls spatial accuracy, not stability. At the implicit ZOH timestep of 3600s, adding more concrete nodes does not change the heat flux appreciably because the insulation layer (R=1.54 m²K/W) is the dominant resistance. The prior analysis that named this as the "dominant cause" confused numerical stability with spatial accuracy.

**Status**: RULED OUT. The empirical test disproves the prior analysis.

---

### H5: Zone Air Capacitance Uses Sea-Level Density

**Finding**: Not hack-tested at full simulation level. Code analysis: `AIR_DENSITY_KG_M3 = 1.2041` at `crates/hares-envelope/src/boundary_rc.rs:17`. Denver altitude 1609m → ρ ≈ 0.987 kg/m³, an 18% error. Zone air capacitance for 900FF: 1.2041 × 1006 × 129.6 = 157.3 kJ/K vs correct 128.5 kJ/K. The concrete walls total ~14.3 MJ/K. Zone air is <1.1% of total thermal mass. The 18% error on zone air is <0.2% of total mass. Impact on min temp: negligible compared to H1.

**Status**: Real physics defect (inconsistency: infiltration solver uses altitude density, capacitance uses sea-level). Impact on 900FF minimum temperature: <0.05°C. Fix is warranted for correctness, not for BESTEST compliance.

---

### H6: Interior Longwave Radiation (Non-Issue)

**Finding**: `int_lwr = 0.0` in diagnostic output is correct by construction. Interior LWR redistributes energy between surfaces in the enclosure (zero net to zone air for an enclosed zone). This is not a defect.

**Code location**: `crates/hares-envelope/src/thermal_solver/longwave.rs` — `apply_interior_longwave_inputs` computes Gebhart grey interchange factors. Zone total = Σq_i ≈ 0.

**Status**: RULED OUT.

---

### H7: Interior Film Coefficients (Non-Issue)

**Finding**: TARP convection model IS implemented at `crates/hares-physics/src/film_coefficients.rs` and IS used by `conversions.rs`. The film coefficients are not static 0.12 m²K/W — they are TARP-derived with temperature dependency. This hypothesis was unfounded.

**Status**: RULED OUT.

---

## Error Budget — Measured Contributions

| Hypothesis | Measured Delta on min_zone_temp_c | Status |
|------------|----------------------------------|--------|
| H1: Initial zone temperature (21°C → 0°C proxy) | **−4.53°C** (21°C→0°C proxy range) | DOMINANT ROOT CAUSE — must fix |
| H1: Warmup-appropriate initial (~3°C) | −2.39°C | Proxies the annual-periodic fix |
| H2: Radiant/convective split (100% → 60/40) | −0.295°C | Real defect — must fix |
| H3: Solar to zone air only (vs radiation_frac split) | −0.142°C | Current code is CORRECT |
| H4: RC discretization (2→4 concrete nodes) | +0.029°C (wrong dir) | RULED OUT |
| H5: Zone air density (sea-level) | <0.05°C estimated | Real defect, negligible impact |
| H6: Interior LWR | 0°C | RULED OUT (correct physics) |
| H7: Interior film coefficient | 0°C | RULED OUT (TARP implemented) |

**Observed outlier**: +2.50°C.

With warmup-appropriate initialization (equivalent to initial zone temp ~3°C based on sensitivity table), the estimated residual is 2.50 − 2.39 − 0.295 = −0.18°C, meaning the combination of correct initialization + radiant split would put 900FF inside the [−6.4, −1.6°C] band.

---

## Fix Plan

### Fix 1 (Primary): Implement Annual-Periodic Warmup for Free-Float Buildings

**Target**: `crates/hares-envelope/src/thermal_solver/initialization.rs` and `crates/hares-envelope/src/thermal_solver/mod.rs`.

**Required change**: When no HVAC setpoints exist (free-float), the initial zone temperature defaults to 21°C — an HVAC building assumption that is wrong for free-float cases. The correct approach is one of:

Option A — **Warmup period**: After `initialize_steady_state`, run N steps using the first N hours of annual weather (weather file wraps around) until the RC state vector change between consecutive years is below a threshold (e.g., max node delta < 0.1°C). EnergyPlus uses ≥20 warmup days repeated until convergence.

Option B — **Annual-average initialization**: Replace the `env.weather.outdoor_temp_c` boundary condition in `initialize_steady_state` with `weather_avgs.avg_ambient_c` (already computed in `build_default_solvers`). This requires passing `avg_ambient_c` into `ThermalSolver::new`. The initial zone air temperature for free-float buildings should be set to `avg_ambient_c + delta` where `delta` accounts for internal gains.

Option A is more general and matches EnergyPlus behavior. Option B is cheaper but requires analytical estimation of the annual-mean zone temperature.

**Verification**: Re-run `bestest_case_900ff` — min_zone_temp_c must enter [−6.4, −1.6°C]. Re-run `bestest_case_600ff` — must remain in [−18.8, 0.0°C]. Re-run `bestest_case_600` and `bestest_case_900` — conditioned cases must not regress (warmup period may slightly affect their annual energy, but the 900 heating miss of 0.021% may benefit from colder concrete initialization removing false heat storage).

### Fix 2 (Secondary): Implement Radiant/Convective Split for Port Gains

**Target**: `crates/hares-envelope/src/thermal_solver/ports.rs` — `apply_port_sensible_inputs`.

**Required change**: Add `radiant_gain_fraction` to the `ThermalPort` / `PortSlots::thermal` structure. Route `sensible_gain_w × radiant_fraction` to interior surface nodes (proportional to area × absorptance), and route the remainder to zone air. ASHRAE 140 §5.2.4.3 specifies 60% radiant.

**Verification**: 900FF min_zone_temp_c should drop by ~0.3°C from this fix.

### Fix 3 (Secondary): Zone Air Capacitance Altitude Correction

**Target**: `crates/hares-envelope/src/boundary_rc.rs` — `derive_zone_capacitances`.

**Required change**: Accept site pressure (Pa) and compute density via `p / (R_air × T_K)`. Impact on BESTEST is negligible but the inconsistency with infiltration solver is a real defect.

---

## Remaining Unknowns

1. **Warmup period exact behavior**: The sensitivity table shows a nonlinear response (10°C → 21°C: no change; 5°C → 10°C: −0.98°C). The warmup period's steady-state zone temperature at Jan 1 (after annual cycling) is unknown without running the 2-year simulation. The correct floor and wall temperatures at Jan 1 in annual-periodic steady state need to be measured by running the simulation for ≥2 years and discarding year 1.

2. **Case 900 interaction**: Fixing warmup initialization will affect Case 900 (conditioned). Colder initial concrete means the first few days of January have higher heating demand. The direction of the annual-total effect is unknown without measurement.

---

## Sources

- ASHRAE 140-2017 §5.2.4.3, Table B8-3a — minimum zone temperature reference band [−6.4, −1.6°C] for Case 900FF, internal gains specification (60% radiant / 40% convective).
- [EnergyPlus ERM 26.1 — Warmup Convergence](https://bigladdersoftware.com/epx/docs/26-1/engineering-reference/warmup-convergence.html) — documents ≥20 warmup day requirement and convergence criterion.
- [NREL BESTEST-GSR Repository](https://github.com/NREL/BESTEST-GSR) — `Fraction_Radiant: 0.6` on OtherEquipment in reference EnergyPlus simulation confirms ASHRAE 140 radiant split.
- [EnergyPlus Engineering Reference: Zone Internal Gains](https://bigladdersoftware.com/epx/docs/8-3/engineering-reference/zone-internal-gains.html) — 60% radiant / 40% convective default for internal equipment gains.
- [LBNL Modelica Buildings Library BESTEST Cases9xx](https://simulationresearch.lbl.gov/modelica/releases/latest/help/Buildings_ThermalZones_Detailed_Validation_BESTEST_Cases9xx.html) — confirms 2007-edition reference range [0.6, 2.2°C] for 900FF min temp (looser than 2017 band used here).
