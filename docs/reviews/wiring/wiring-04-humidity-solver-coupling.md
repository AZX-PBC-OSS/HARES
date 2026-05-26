# Humidity solver coupling: thermal->humidity latent payload alignment
**Review ID**: wiring-04
**Category**: wiring
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/humidity_solver.rs`
- `crates/hares-envelope/src/thermal_solver/stepping.rs`
- `crates/hares-core/src/dwelling/mod.rs`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/HumidityManager.cc` — **not found** in repository. Review relies on EnergyPlus Engineering Reference documentation cited in HARES source comments.

## Findings

### Finding 1: [Severity: medium]
**Description**: Humidity ratio clamping to saturation (`w_new.clamp(0.0, w_sat)`) silently discards the latent heat of condensation. When the moisture balance would produce `w_new > w_sat`, the solver clamps `w_new` to `w_sat`, discarding the excess moisture mass and — critically — never communicating the associated latent heat release back to the thermal solver. This creates an energy imbalance: in a condensing scenario, the zone should experience sensible heating from the phase-change energy, but the thermal solver is unaware of the event.

**Code Location**:
- Clamping: `humidity_solver.rs:282-283`
- Invariant skip (acknowledging mass loss): `dwelling/mod.rs:3332-3343` — the moisture balance invariant explicitly skips clamped zones with the comment "Clamping breaks mass conservation by design."

**Root Cause**: The humidity solver is a one-way consumer of thermal outputs. There is no feedback mechanism (e.g., a per-zone condensation latent gain written back to the thermal ports or `custom_payload`) that would let the thermal domain account for latent heat release from equilibrium condensation. The `ThermalCategory::HvacDehumidification` category exists for equipment-driven dehumidification but is not used for equilibrium condensation.

**Impact**:
- In high-humidity scenarios where the zone temperature is near the dew point, condensation energy is silently lost — the simulation underestimates zone sensible temperature.
- The invariant checker catches the mass imbalance but deliberately suppresses the warning, so the issue is invisible at runtime.
- For residential simulations in typical conditions (indoor ~20°C, 50% RH), condensation is rare unless latent gains are very high, limiting practical impact. However, in hot/humid climates with high infiltration of moist outdoor air and tight enclosures, this could become significant.

**EnergyPlus Comparison**: EnergyPlus's `HumidityManager` explicitly computes zone moisture condensation mass and adds the latent heat release back to the zone heat balance. The HARES humidity solver, while citing the EnergyPlus Engineering Reference "Moisture Predictor-Corrector" section for the infiltration coupling, omits the condensation feedback step that EnergyPlus includes.

### Finding 2: [Severity: medium]
**Description**: The moisture balance invariant checker in `dwelling/mod.rs:3380-3381,3385` hardcodes the latent heat of vaporisation as the literal `2_501_000.0` J/kg, but the humidity solver's latent heat is configurable via `HumiditySolverConfig.h_fg_j_kg` (defaulting to `LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J = 2_501_000.0`). If the config constant is changed (e.g., to 2_454_000 for OCHRE parity tests), the invariant checker would produce false-positive moisture balance violations because it uses a different `h_fg` than the solver.

**Code Location**:
- Hardcoded `2_501_000.0` in invariant: `dwelling/mod.rs:3380` (`m_dot_inf * 2_501_000.0`), `dwelling/mod.rs:3381` (`q_other * dt_s / 2_501_000.0`), `dwelling/mod.rs:3385` (`actual_source * 2_501_000.0`)
- Configurable `h_fg_j_kg` in solver: `humidity_solver.rs:29,35,36`
- Thermal solver's own const: `thermal_solver/mod.rs:63` — `H_FG_J_PER_KG` is a separate `const` defined from the same physics constant

**Root Cause**: The invariant checker does not read `self.humidity_solver.config.h_fg_j_kg` for its moisture balance arithmetic. It uses a literal that happens to match the default but would diverge if the default is overridden.

**Impact**: Low practical impact today (both constants resolve to 2,501,000 J/kg by default), but a latent maintenance risk. The test at `humidity_solver.rs:705-716` (`thermal_and_humidity_solvers_share_latent_heat_constant`) verifies the equality, but only for the default config and only at test time — not at runtime with a custom config.

**Recommendation**: Replace the three occurrences of `2_501_000.0` with `self.humidity_solver.config.h_fg_j_kg`.

### Finding 3: [Severity: low]
**Description**: The thermal-to-humidity payload format is consistently aligned across all readers and writers. The 5-float format `[zone_id, q_latent_w, m_dot_inf_kg_s, w_outdoor, energy_balance_residual_w]` is written by `thermal_solver/mod.rs:862-870` and read identically by:
- `humidity_solver.rs:132-137`
- `dwelling/mod.rs:3300-3308` (invariant checker)
- `dwelling/conversions.rs:486-491` (humidity-to-zone update, which reads the humidity output format — a separate 5-float format, also consistent)

A prior bug with `chunks_exact(4)` misaligning multi-zone payloads was documented and fixed (see comment at `humidity_solver.rs:559-562` and the regression test `multi_zone_thermal_payload_parses_each_zone_independently` at line 566). No silent data swap risk remains.

**Impact**: None — this is a positive finding confirming alignment.

### Finding 4: [Severity: low]
**Description**: Moisture buffering by building materials is modeled as a simple effective capacitance multiplier (`moisture_buffering_multiplier`, default 15×) rather than a full sorption/isotherm model. This multiplies the effective air moisture capacitance uniformly across all zones. The approach matches OCHRE's `humidity_cap_mult` but is explicitly documented as "single-zone validated only" (`humidity_solver.rs:21`).

**Code Location**: `humidity_solver.rs:23,34`

**Impact**: The 15× multiplier captures first-order moisture buffering (slower RH swings, reduced dehumidifier cycling) but does not model:
- Material-dependent sorption isotherms (gypsum vs. wood vs. concrete)
- Hysteresis between adsorption and desorption
- Inter-zone moisture transport through permeable materials
- Temperature dependence of sorption capacity

The documentation is clear about the limitation. This is a known simplification, not a defect.

### Finding 5: [Severity: low]
**Description**: In the humidity solver's semi-implicit infiltration path (`humidity_solver.rs:215`), `w_outdoor` falls back to `0.0` via `unwrap_or(0.0)` when the zone's entry is missing from `w_outdoor_buf`. The insertion guard at line 144 (`if w_outdoor > 0.0`) prevents zero-valued outdoor humidity ratios from entering the map. However, if the latent payload for a zone somehow carries `m_dot_inf > 0.0` and `w_outdoor = 0.0`, the semi-implicit path activates with a physically incorrect outdoor humidity ratio of zero, driving the zone humidity ratio toward zero instead of the true outdoor value.

**Code Location**: `humidity_solver.rs:144-146` (insert guard) and `humidity_solver.rs:215` (fallback)

**Impact**: In practice, this cannot be triggered in the current code because `w_outdoor` is sourced from `env.weather.outdoor_humidity_ratio` which is always > 0 in real weather data, and the thermal solver always includes the outdoor humidity ratio from the weather. However, if the humidity solver were ever driven by synthetic data with `w_outdoor = 0.0` and `m_dot_inf > 0.0`, the zone humidity would incorrectly collapse to zero. The guard at line 144 is a second line of defense but masks the inconsistency — if `w_outdoor = 0.0` is unexpected, the solver should at minimum log a warning rather than silently using `0.0`.

**Recommendation**: Add a `tracing::warn!` when `has_infiltration_coupling` is true but `w_outdoor_buf` is missing for the zone.

## Summary
- Total findings: 5
- Critical: 0
- High: 0
- Medium: 2 (clamping energy loss, hardcoded h_fg in invariant checker)
- Low: 3 (payload alignment confirmed positive, moisture buffering documentation, w_outdoor fallback guard)

## Recommendations
1. **Medium**: Implement a condensation latent-heat feedback mechanism: when the humidity solver clamps `w_new` to `w_sat`, compute the excess moisture mass (`w_new - w_sat`), convert to latent energy (`delta_w * rho * V * h_fg`), and write it back as a sensible gain to the zone thermal ports under `ThermalCategory::HvacDehumidification` (or a new `Condensation` category). This follows the EnergyPlus pattern where `MoisturePredictorCorrector` accounts for condensation in the zone heat balance.

2. **Medium**: Replace the hardcoded `2_501_000.0` in the invariant checker (`dwelling/mod.rs:3380,3381,3385`) with `self.humidity_solver.config.h_fg_j_kg` to ensure the moisture balance check uses the same latent heat constant as the solver it validates.

3. **Low**: Add a `tracing::warn!` in the humidity solver when `has_infiltration_coupling` is true but `w_outdoor_buf` lacks an entry for the zone (around `humidity_solver.rs:215`), so that a broken coupling is visible rather than silently degrading to `w_outdoor = 0.0`.

4. **Low**: Consider upgrading the moisture buffering model from a scalar capacitance multiplier to a material-dependent sorption model (e.g., EMPD — Effective Moisture Penetration Depth) for multi-zone buildings with diverse interior finishes. This is a feature request, not a defect.

## References / Citations
- EnergyPlus Engineering Reference (2024), "Moisture Predictor-Corrector" — cited in HARES comments for semi-implicit infiltration latent coupling (`thermal_solver/infiltration.rs:24-29`, `humidity_solver.rs:210-213`)
- ASHRAE HoF 2021, Ch.1 Eq.28 — specific volume / moist air density formulation referenced in `thermal_solver/infiltration.rs:91-92`
- OCHRE `humidity_cap_mult` (15×) — referenced as the source for `moisture_buffering_multiplier` default in `humidity_solver.rs:21,34`
- EnergyPlus Engineering Reference, "Basis for the Zone and Air System Integration" — cited at `stepping.rs:582-596` for energy balance closure methodology
- Walton (1983), NBSSIR 83-2655 — TARP interior convection model referenced in `stepping.rs:528` for boundary diagnostic flow calculations
