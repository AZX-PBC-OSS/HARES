# Infiltration Uses Raw Met-Station Wind Speed Without Terrain/Height Correction

**Severity**: High
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/thermal_solver/infiltration.rs, hares-physics/infiltration.rs

## Problem

`apply_infiltration_and_ventilation` (`crates/hares-envelope/src/thermal_solver/infiltration.rs:96, 109, 142`)
passes `env.weather.wind_speed_m_s` directly to `ashrae_wind_stack`, `ela_infiltration`,
and `natural_ventilation_flow_m3_s` without any terrain or height correction.

EPW and weather file wind speeds are measured at 10 m above open-country terrain
(ASHRAE standard meteorological station conditions). The wind speed at the building
envelope in suburban, urban, or rural terrain differs by the power-law profile:

```
u_building = u_met × (δ_met / H_met)^α_met × (H_building / δ_site)^α_site
```

Per ASHRAE HoF 2021, Ch. 24, Table 1 (Outdoor Design Conditions — Wind Speed
Adjustment) and Walker & Wilson 1998, this correction must be applied before
computing wind-driven infiltration terms. The net effect of omitting it is
30–50% overestimation in urban terrain and up to 20% underestimation in exposed
rural terrain.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/infiltration.rs:96`:

```rust
ashrae_wind_stack(c_s, c_w, delta_t, env.weather.wind_speed_m_s, shielding_coeff, n_i)
```

`crates/hares-envelope/src/thermal_solver/infiltration.rs:109`:

```rust
ela_infiltration(ela_m2, stack_coeff, wind_coeff, delta_t, env.weather.wind_speed_m_s)
```

The terrain correction is fully implemented in `hares_physics::infiltration::terrain_wind_speed`
and `terrain_wind_speed_for_class` (with correct constants: `MET_STATION_ALPHA = 0.14`,
`MET_STATION_DELTA_M = 270.0 m`, `MET_STATION_HEIGHT_M = 10.0 m`) and is tested
in isolation but never called from the stepping path.

**AshraeWindStack double-correction risk.** The `Aim2Coefficients` initialisation
(`crates/hares-physics/src/infiltration.rs:526–535`) bakes terrain correction
into `shelter_coeff` via `terrain_wind_speed(1.0, ...)`. When the stepping code
subsequently applies terrain correction to the raw met-station wind, the correction
would be applied twice for the `AshraeWindStack` branch. The fix must correct the
`Ela` branch unconditionally and document clearly that `AshraeWindStack` wind
coefficients already embed terrain correction and must not receive a second
correction at step time.

OCHRE also passes uncorrected wind speed; OCHRE is the floor, not the target.

## Required Behavior

Per ASHRAE HoF 2021, Ch. 24 § "Wind data correction" and Walker & Wilson 1998
(AIM-2 model), the wind speed at the building envelope must be terrain- and
height-corrected before use in any infiltration model. The `Ela` branch must
apply this correction. The `AshraeWindStack` branch must document that its
pre-computed coefficients already embed it.

## Approach

1. Add `terrain_class: TerrainClass` and `building_height_m: f64` as fields on
   `ThermalSolverConfig` (shared across infiltration methods).
2. In `apply_infiltration_and_ventilation`, for the `Ela` branch only, compute:
   ```rust
   let u_site = terrain_wind_speed(
       env.weather.wind_speed_m_s,
       terrain_class.alpha(),
       terrain_class.delta_m(),
       building_height_m,
   );
   ```
   and pass `u_site` instead of `env.weather.wind_speed_m_s`.
3. For `AshraeWindStack`, pass `env.weather.wind_speed_m_s` unchanged (correction
   already embedded in coefficients). Add an inline comment explaining this invariant.
4. Do not apply the correction in `natural_ventilation_flow_m3_s` unless its
   derivation is confirmed to require it separately.

## Definition of Done

- [ ] `terrain_class` and `building_height_m` available in `ThermalSolverConfig`.
- [ ] `Ela` branch in `apply_infiltration_and_ventilation` applies `terrain_wind_speed`
      before calling `ela_infiltration`.
- [ ] `AshraeWindStack` branch passes uncorrected wind speed with a comment confirming
      the double-correction invariant.
- [ ] Test: suburban terrain (α=0.22, δ=370 m) at 8 m height produces wind speed
      ≈62% of met-station value and correspondingly lower `Ela` infiltration.

## Verification

```bash
cargo test -p hares-envelope infiltration
cargo test -p hares-physics terrain_wind_speed
```

Expected: `terrain_wind_speed(u_met, 0.22, 370.0, 8.0)` ≈ 0.62 × u_met
(suburban at 8 m per ASHRAE HoF Ch. 24 Table 1).

## References

- ASHRAE Handbook of Fundamentals 2021, Ch. 24 § "Wind data correction for
  terrain and height" (power-law profile, Table 1 terrain parameters).
- ASHRAE Handbook of Fundamentals 2021, Ch. 16 §4 (Air leakage — terrain-corrected
  wind speed in AIM-2 model).
- Walker, I.S. and Wilson, D.J. (1998), "Field Validation of Algebraic Equations
  for Stack and Wind Driven Air Infiltration Calculations," HVAC&R Research 4(2):
  119–139.
- EnergyPlus Engineering Reference §15.4 (AIM-2 Infiltration Model — terrain-
  corrected wind speed).

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation
- [x] Referenced line numbers still match (corrected locations noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: matches — OCHRE also passes raw `wind_speed` to both
      `_infiltration_ashrae` and `_ela` at runtime (vendors/OCHRE/ochre/Models/Envelope.py,
      `_infiltration_ashrae` line ~23, `Zone.update_infiltration` line ~584/596). Terrain
      correction is pre-baked into `inf_sft` / `wind_coeff` at param-setup time, same as HARES.
- [x] EnergyPlus cross-check result: matches — EnergyPlus corrects wind speed for terrain
      and height globally via `Site:HeightVariation` / Building Terrain field before it reaches
      any infiltration model. The ELA formula as documented uses "local wind speed" (already
      terrain-corrected upstream). HARES passes EPW raw wind speed with no such upstream
      correction. Diverges from EnergyPlus intent.

### Code Location Corrections

The ticket cites three call sites. Actual current line numbers (verified by reading
`crates/hares-envelope/src/thermal_solver/infiltration.rs`):
- `ashrae_wind_stack(...)` call: **line 92–99** (not 96 as stated)
- `ela_infiltration(...)` call: **line 104–110** (not 109 as stated)
- `natural_ventilation_flow_m3_s(...)` call: **line 135–147** (not 142 as stated)

The ticket's lines 96/109/142 are off by a few lines due to code churn, but the
described logic (raw `env.weather.wind_speed_m_s` passed in each case) is confirmed.

### Critical Correction: Both Branches Pre-bake Terrain Correction

The ticket incorrectly identifies only the `AshraeWindStack` branch as having
terrain correction pre-baked into its coefficients. **Both** branches pre-bake it:

- `AshraeWindStack`: terrain correction enters `shelter_coeff` via
  `terrain_wind_speed(1.0, ...)` in `aim2_coefficients_from_ach50`
  (`crates/hares-physics/src/infiltration.rs:527–532`). Confirmed by ticket.

- `Ela`: terrain correction enters `wind_coeff` via `f_t` in
  `calculate_ela_coefficients` (`crates/hares-physics/src/infiltration.rs:622–623`).
  This function is the canonical ELA setup path used by `solver_builder.rs:1155` and
  `solver_builder.rs:1205`. **The ticket does not mention this and incorrectly states
  the Ela branch "must apply terrain correction" at runtime.**

Consequence: The proposed fix in the ticket is wrong in direction. Applying
`terrain_wind_speed()` to `env.weather.wind_speed_m_s` before passing it to
`ela_infiltration()` would *introduce* a double-correction (terrain already in coeff),
making things worse. **Neither branch should receive a separately terrain-corrected
wind speed at step time.** The true bug is that the coefficients are computed as if
they will receive raw met-station wind speed, which is mathematically consistent, but
the EPW wind is then used unchecked against the implicit assumption embedded in the
coefficients (that the met station is the wind measurement source with 10 m / rural
terrain). This is a documentation/invariant gap, not a runtime correction gap.

### Numeric Claim Correction

The ticket states: `terrain_wind_speed(u_met, 0.22, 370.0, 8.0) ≈ 0.62 × u_met`.

**Actual computed value: 0.6824** (verified by calculation and pinned by
`ticket028_suburban_8m_correction_factor` test). The 0.62 figure is incorrect.

### Web-Verified Citations

**Citation 1**: ASHRAE HoF 2021, Ch. 24, Table 1 (Outdoor Design Conditions —
Wind Speed Adjustment)
- **Source found**: ASHRAE Handbook of Fundamentals 2017 Ch. 24 online
  (handbook.ashrae.org/Handbooks/F17/IP/f17_ch24) and IES VE help citing ASHRAE 2001
- **Quoted passage**: "The hourly average wind speed UH in the undisturbed wind
  approaching a building in its local terrain can be calculated from Umet as follows
  [power-law formula]." Table 1 lists: Open terrain (a=0.14, δ=270 m), Suburban
  (a=0.22, δ=370 m), Urban (a=0.33, δ=460 m).
- **Verdict**: **Partially correct.** The table and terrain parameters are confirmed.
  However, Ch. 24 is titled "Airflow Around Buildings", not "Outdoor Design
  Conditions" as stated in the ticket. The ticket's parenthetical "Outdoor Design
  Conditions — Wind Speed Adjustment" is a mis-label; that topic is wind-driven
  pressure/flow around buildings, not a standalone design conditions chapter.

**Citation 2**: ASHRAE HoF 2021, Ch. 16 §4 (Air leakage — terrain-corrected wind
speed in AIM-2 model)
- **Source found**: ASHRAE Handbook of Fundamentals 2017 Ch. 16 online
  (handbook.ashrae.org/Handbooks/F17/SI/f17_ch16)
- **Quoted passage**: "The reference wind speed used to determine pressure
  coefficients is usually the wind speed at the eave height for a low-rise building
  … The difference in terrain between the measurement station and the building under
  study must also be addressed. Chapter 24 shows how to calculate the effective wind
  speed UH from the reference wind speed Umet using boundary layer theory and
  estimates of terrain effects."
- **Verdict**: **Confirmed in substance** — Ch. 16 does discuss terrain-corrected
  wind speed for infiltration and defers to Ch. 24 for the formula. Section §4 as a
  specific subsection number could not be independently verified (requires paywalled
  access), but the general claim is supported.

**Citation 3**: Walker & Wilson (1998), HVAC&R Research 4(2): 119–139
- **Source found**: AIVC preprint LBNL-42361, aivc.org/sites/default/files/airbase_11869.pdf
- **Quoted passage**: "AIM-2 combines ideas from previous ventilation models …
  with new concepts … Also added are additional refinements regarding wind shelter
  calculations, and adjusting wind speeds from the measurement site to the building.
  … Walker and Wilson (1990b) showed how meteorological windspeeds measured remotely
  from the building site can be converted to an eaves height windspeed at the
  building, assuming a power law boundary layer wind velocity profile."
- **Verdict**: **Confirmed.** The paper does define how met-station wind speed is
  converted via power-law profile. However, in AIM-2 this conversion is embedded
  *in the shelter coefficient* during coefficient setup — it is not an independent
  runtime correction to the wind speed argument. The ticket conflates the two.

**Citation 4**: EnergyPlus Engineering Reference §15.4 (AIM-2 Infiltration Model —
terrain-corrected wind speed)
- **Source found**: EnergyPlus Engineering Reference v23.2 (bigladdersoftware.com/
  epx/docs/23-2/engineering-reference/infiltration-ventilation.html)
- **Quoted passage**: EnergyPlus ELA model formula uses "local wind speed [m/s]".
  EnergyPlus docs note that the local wind speed is adjusted globally via terrain
  classification (Building Terrain field / Site:HeightVariation object) before
  reaching infiltration models.
- **Verdict**: **Partially correct.** The claim that EnergyPlus applies
  terrain-corrected wind speed to its AIM-2/ELA models is confirmed. However, the
  specific section "§15.4" was not found in current EnergyPlus docs (the document
  does not use numerical section identifiers in that format). The substance of the
  citation is correct; the section number cannot be verified.

### Legitimacy
- **Verdict**: **Partially Legitimate**
- **Rationale**: The core observation is real — the HARES stepping path passes raw
  EPW wind speed directly to both infiltration models, while EnergyPlus applies a
  terrain/height correction upstream. However, the ticket's diagnosis and proposed
  fix are wrong in a critical way: `calculate_ela_coefficients` already pre-bakes
  terrain correction into `wind_coeff` (confirmed at
  `crates/hares-physics/src/infiltration.rs:622–623`), exactly as
  `aim2_coefficients_from_ach50` does for `shelter_coeff`. Applying an additional
  `terrain_wind_speed()` correction at step time to the `Ela` branch (as the ticket
  proposes) would produce a double-correction, not a fix. The actual issue is an
  *invariant documentation gap*: both branches expect raw met-station wind speed at
  runtime, and that invariant is not documented anywhere in the stepping code. The
  impact estimate (30–50% urban overestimation) is also overstated; since terrain
  correction is already embedded in the pre-computed coefficients, the models are
  internally consistent — the residual error is from EPW wind speed not being exactly
  at the met-station reference height/terrain assumed in coefficient calculation.
  Additionally, the 62% numeric claim is wrong (actual: 68.2%).

### Proposed Fix Summary

Do NOT add a runtime `terrain_wind_speed()` call in `apply_infiltration_and_ventilation`.
Instead:
1. Add inline comments in `apply_infiltration_and_ventilation` documenting the
   invariant: both `AshraeWindStack` and `Ela` coefficients already embed terrain
   correction (via `shelter_coeff` and `wind_coeff` respectively), so both branches
   must receive raw met-station wind speed.
2. Add a comment in `calculate_ela_coefficients` and `aim2_coefficients_from_ach50`
   stating that the output coefficients are calibrated for raw EPW/met-station wind
   speed input (10 m, open-country terrain).
3. Correct the `~62%` figure in the ticket to `~68%` (or remove the approximation).
4. If there is a genuine concern that the EPW wind speed height differs from the
   assumed met-station reference height of 10 m (some EPW files document measurement
   heights other than 10 m), that should be tracked as a separate, narrower ticket
   covering EPW metadata parsing.

### Test Written
- File: `crates/hares-physics/src/infiltration.rs` (within `#[cfg(test)] mod tests`)
- Tests added:
  1. `ticket028_suburban_8m_correction_factor` — pins the correct value (0.6824, not
     0.62) for suburban terrain at 8 m per ASHRAE HoF Ch. 24 Table 1.
  2. `ticket028_ela_wind_coeff_embeds_terrain_correction` — demonstrates that
     `calculate_ela_coefficients` pre-bakes terrain correction into `wind_coeff`, so
     applying a second terrain correction at runtime would understate infiltration.
  3. `ticket028_aim2_shelter_coeff_embeds_terrain_correction` — demonstrates that
     `aim2_coefficients_from_ach50` pre-bakes terrain correction into `shelter_coeff`,
     confirming both branches have the same invariant.
