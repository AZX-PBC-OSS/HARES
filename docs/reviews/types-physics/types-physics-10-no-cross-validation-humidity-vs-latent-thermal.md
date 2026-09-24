# No cross-validation humidity vs latent thermal ports
**Review ID**: types-physics-10
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/humidity_solver.rs`
- `crates/hares-envelope/src/thermal_solver/mod.rs`
- `crates/hares-envelope/src/thermal_solver/infiltration.rs`
- `crates/hares-envelope/src/thermal_solver/stepping.rs`
- `crates/hares-equipment/src/ports.rs`
- `crates/hares-types/src/ports.rs`
- `crates/hares-equipment/src/hvac/dehumidifier.rs`
- `crates/hares-equipment/src/hvac/air_conditioner.rs`
- `crates/hares-equipment/src/hvac/ideal_hvac.rs`
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs`
- `crates/hares-equipment/src/hvac/duct_distribution.rs`
- `crates/hares-core/src/dwelling/mod.rs`
- `crates/hares-core/src/invariants.rs`

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: critical] No iteration between thermal and humidity solvers within each timestep
**Description**: The humidity and thermal domains are thermodynamically coupled (latent heat exchange affects zone temperature via the heat balance; zone temperature affects saturation vapor pressure which drives moisture transfer). EnergyPlus uses a "Moisture Predictor-Corrector" method (Engineering Reference, "Moisture Predictor-Corrector") that iterates between the heat balance and moisture balance at each timestep until both converge. HARES runs the two solvers in a single sequential forward pass — thermal first, then humidity — with no iteration or feedback.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2649-2678`. `thermal_solver.integrate()` runs at line 2649, results are applied to zone state at line 2653, upserted at line 2660, and `humidity_solver.resolve()` runs at line 2673. No loop or convergence check follows.

**Root Cause**: The solver orchestration treats the humidity and thermal domains as separable, operating on a one-pass assumption that the thermal domain's updated temperature and latent payload are sufficient for a single-step humidity solve. The semi-implicit infiltration coupling in `humidity_solver.rs:206-250` reduces instability from explicit forward-Euler, but does not iterate.

**Impact**: Without iteration, the humidity ratio computed in the current timestep uses `t_zone_c` from the thermal solver's converged state, but the thermal solver's latent heat terms (infiltration plus port latent gains) affect the NEXT timestep's temperature — there is no feedback within the same step. During large transients (e.g., a dehumidifier turning on), the moisture removal's latent heat effect does not cool the zone within the same timestep. The error magnitude is approximately `Q_latent * dt / C_zone` (temperature) or roughly `rho * V * cp * dT / (h_fg * rho * V)` times the W ratio delta, which for a typical 200 m^3 zone at 60 s timestep is ~0.003 K per 100 W latent, accumulating per timestep until equilibrium is reached.

This was also explicitly noted in the infiltration latent coupling comment at `humidity_solver.rs:208-213`: it references the EnergyPlus "Moisture Predictor-Corrector" but implements only the semi-implicit infiltration denominator, not the full heat/moisture iteration.

---

### Finding 2: [Severity: high] HPWH writes latent gain to Thermal port without corresponding Humidity port
**Description**: The heat pump water heater (`heat_pump_wh.rs:745-759`) writes `latent_gain_w` to the `Thermal` port accumulator using `ThermalCategory::HvacDehumidification`. However, it does NOT write a `Humidity` port with `moisture_mass_flow_kg_s`. This is inconsistent with all other equipment that writes latent gains: the dehumidifier (`dehumidifier.rs:361-378`), air conditioner (`air_conditioner.rs:803-819`), and IdealHvac (`ideal_hvac.rs:569-584`) all emit both a `Thermal` port (with `latent_gain_w`) AND a `Humidity` port (with `moisture_mass_flow_kg_s`).

**Code Location**: `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:745-759` — only a `Thermal` contribution is emitted; no `Humidity` contribution follows.

**Root Cause**: The original ticket (001-unify-hfg-add-humidity-port.md) required equipment to emit explicit moisture mass-flow rates, and this was implemented for dehumidifier, AC, and IdealHvac but missed for the HPWH's dehumidification path. The HPWH's latent extraction from the zone air follows the same physics as a standalone dehumidifier.

**Impact**: The humidity solver falls back to the `latent_gain_w / h_fg` conversion path for HPWH latent contributions (`humidity_solver.rs:265-280`). This path is explicitly bypassed when a `moisture_mass_flow_kg_s` value is available (preferred path at line 251). It works correctly because both the HPWH and the humidity solver use `LATENT_HEAT_VAPORISATION_0C_KJ_KG` — but if either's h_fg constant were to change independently, the fallback path would produce inconsistent moisture mass removal. More importantly, the humidity solver's `moisture_mass_flow_kg_s` path is the "golden" path that avoids unit conversion altogether; the HPWH bypasses it, creating a latent quality-of-implementation gap.

---

### Finding 3: [Severity: high] No runtime cross-validation that Thermal latent_gain_w and Humidity moisture_mass_flow_kg_s are thermodynamically consistent
**Description**: When equipment writes both a `Thermal { latent_gain_w }` and a `Humidity { moisture_mass_flow_kg_s }` contribution, there is no runtime check that `latent_gain_w ≈ moisture_mass_flow_kg_s * h_fg`. A bug in equipment code (e.g., using a different h_fg value in one path vs the other, or computing latent gain and moisture mass flow from different snapshots of zone conditions) could produce inconsistent ports without any detection.

**Code Location**: 
- Equipment writes both ports: `air_conditioner.rs:803-819`, `dehumidifier.rs:361-378`, `ideal_hvac.rs:569-584`.
- Humidity solver accumulates both paths independently: `humidity_solver.rs:160-173`.
- Invariant checks validate moisture mass balance (`dwelling/mod.rs:3270-3389`) but only against `latent_from_ports` (thermal accumulator); they do not compare thermal port latent values against humidity port mass-flow values reconverted to latent energy.

**Root Cause**: The two port types trace separate code paths. The `Thermal` accumulator's `latent_gain_w` is summed per zone from all `PortContribution::Thermal` entries; the `Humidity` accumulator's `moisture_mass_flow_kg_s` is summed from all `PortContribution::Humidity` entries. The humidity solver reads both accumulators independently and only uses `moisture_mass_flow_kg_s` as a "preferred" source when non-zero, deferring to `latent_gain_w / h_fg` as a fallback. No cross-check exists between the two.

**Impact**: Thermodynamically inconsistent port data would silently produce incorrect moisture balance. For example, if a future AC model computes `latent_gain_w` at one evaporator condition and `moisture_mass_flow_kg_s` at another, the humidity solver would apply the wrong moisture removal rate via the preferred path while the thermal solver would apply a different latent energy magnitude.

---

### Finding 4: [Severity: medium] No Lewis relation between convective heat and mass transfer coefficients
**Description**: In a condensing-surface model (e.g., a cooled window or wall surface where water vapor condenses), sensible heat transfer and moisture transfer occur simultaneously through the same boundary layer. The Lewis relation (`h / (ρ * cp * h_m) ≈ 1` for air-water mixtures) links the convective heat transfer coefficient to the convective mass transfer coefficient. HARES has no implementation of this relation anywhere in the codebase — there is no `Lewis`, `Chilton-Colburn`, or heat-mass analogy code.

**Code Location**: A full-text search for `Lewis`, `lewis`, `chilton`, `colburn`, `heat.*mass.*analogy`, or `mass.*transfer.*coefficient` across all `.rs` files in the workspace returned zero matches.

**Root Cause**: HARES does not model surface condensation. Moisture transfer is modeled only as:
1. Bulk air exchange (infiltration/ventilation) where sensible and latent are computed from separate volume flow rates (`infiltration.rs:203-236`: `sensible_flow_m3_s` and `latent_flow_m3_s` can differ for balanced ventilation with different sensible/latent recovery efficiencies).
2. Explicit moisture injection/removal by equipment via port contributions.

There is no boundary-layer moisture transfer model for interior surfaces (walls, windows). Convective heat transfer at interior surfaces uses the TARP natural convection model (`film_coefficients.rs`) which is purely sensible and has no moisture counterpart.

**Impact**: In a real building, a cold interior surface (e.g., single-pane window at -10 °C outdoor) can act as a dehumidifier by condensing water vapor from the zone air. This simultaneous sensible + latent exchange is absent from the HARES model. For highly glazed buildings in cold climates, this could underestimate latent removal by up to 100–200 W during condensation conditions, depending on interior surface temperature and zone humidity ratio. However, EnergyPlus similarly treats surface condensation as a special case requiring an explicit condensation model; standard residential simulation often omits it. The absence is consistent with OCHRE's scope but diverges from EnergyPlus's full moisture balance.

---

### Finding 5: [Severity: medium] Invariant check hardcodes h_fg instead of using solver config value
**Description**: The moisture mass balance invariant check in `dwelling/mod.rs:3380,3388` hardcodes `2_501_000.0` as the latent heat of vaporization instead of reading `self.humidity_solver.config.h_fg_j_kg`. The same value is used in `invariants.rs:96` (the `check_moisture` method). If the config value is ever changed (e.g., to evaluate sensitivity, or to correct for temperature dependence), the invariant check would report false negative or positive results.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:3380` (`m_dot_inf * 2_501_000.0`) and `crates/hares-core/src/invariants.rs:96` (`const H_FG_J_KG: f64 = 2_501_000.0`).

**Root Cause**: The invariant check was written before the humidity solver exposed its config as a public field, or the constant was inlined for simplicity. The solver's `config.h_fg_j_kg` already defaults to the same value (from `HumiditySolverConfig::default()`) and the thermal solver's `H_FG_J_PER_KG` matches (`LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J`), so this is currently consistent.

**Impact**: Currently zero — all three locations use the identical 2,501,000 J/kg constant. The risk is maintenance: if any future change adjusts the config value, the invariant check silently diverges. The test at `humidity_solver.rs:706-716` explicitly verifies `thermal_h_fg == humidity_h_fg` for the default config, but no test covers non-default config values.

---

### Finding 6: [Severity: low] Moisture mass balance invariant check does not account for Humidity port contributions
**Description**: The invariant moisture balance check at `dwelling/mod.rs:3349-3388` sums `latent_from_ports` (the `ThermalAccumulator::latent_gain_w` totals per zone) and `latent_from_infiltration` (from the thermal domain's custom payload). It does not read or verify the `HumidityAccumulator::moisture_mass_flow_kg_s` values at all. The Humidity port data stream flows through the humidity solver unvalidated against the Thermal port stream.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:3350-3356` — only `self.ports.thermal` is queried; `self.ports.humidity` is never referenced.

**Root Cause**: The invariant check traces the humidity solver's own conversion path (`latent_gain_w / h_fg → delta_w`) to verify mass conservation. Since the solver itself uses `moisture_mass_flow_kg_s` when available (preferred path), the invariant check would miss cases where:
- The humidity solver's preferred path from `moisture_mass_flow_kg_s` produces a different result from the fallback `latent_gain_w / h_fg` path.
- The Humidity accumulator value is produced by buggy equipment code that doesn't match the Thermal accumulator.

**Impact**: Low, because:
1. The three equipment types that write Humidity ports (AC, dehumidifier, IdealHvac) all compute `moisture_mass_flow_kg_s` from the same `latent_gain_w / h_fg` division, so the two paths are mathematically equivalent by construction.
2. The invariant check catches gross energy-conservation violations through the latent path.
3. A cross-port inconsistency would be caught only indirectly — if the humidity solver's preferred path produced a different delta_w than expected, the invariant's mass balance would fail. But if both paths are consistent (they are by code construction), the check passes even if port data were actually wrong.

## Summary
- Total findings: 6
- Critical: 1 | High: 2 | Medium: 2 | Low: 1

## Recommendations
1. **Implement thermal-humidity iteration**: Add a convergence loop in the dwelling timestep that alternates between thermal and humidity solves. EnergyPlus uses the Moisture Predictor-Corrector with a convergence threshold of 0.001 °C on zone temperature and ~1% on humidity ratio. Even a single correction step (predictor → corrector) would capture most of the coupled effect while avoiding the full iteration cost.
2. **Add Humidity port emission to the HPWH**: Emit `PortContribution::Humidity { zone, moisture_mass_flow_kg_s: latent_gain_w / h_fg }` in `heat_pump_wh.rs:759` when `latent_gain_w != 0.0`. This brings the HPWH into parity with the dehumidifier, AC, and IdealHvac.
3. **Add cross-port validation**: In `check_invariants` or in the humidity solver itself, compare `|ports.thermal[zone].latent_gain_w - ports.humidity[zone].moisture_mass_flow_kg_s * h_fg| < epsilon` when both ports are non-zero. Log a warning (not an error) on mismatch since this is a data-quality check, not a physics violation.
4. **Use config h_fg in invariant checks**: Replace hardcoded `2_501_000.0` in `dwelling/mod.rs:3380` and `invariants.rs:96` with `self.humidity_solver.config.h_fg_j_kg`.
5. **Consider surface condensation**: For long-term accuracy, evaluate whether interior surface condensation modeling (via the Lewis relation) would materially affect results. For residential simulations with double-glazed windows and interior surface temperatures rarely below dew point, this may be negligible — but it should be documented as a known limitation.

## References / Citations
- EnergyPlus Engineering Reference (2024), §"Moisture Predictor-Corrector" and §"Basis for the Zone and Air System Integration".
- ASHRAE HoF 2021, Ch.18 "Heat Balance Method" — zone moisture balance requirements.
- EnergyPlus Engineering Reference, "Basis for the Zone and Air System Integration" — iterative thermal-moisture coupling: `C_z dW_z/dt = ... + m_inf*(W_out - W_z) + ...` where heat and moisture equations are solved iteratively.
- Incropera & DeWitt, "Fundamentals of Heat and Mass Transfer" — Lewis relation for air-water mixtures: Le^{2/3} ≈ 1, giving h/h_m ≈ ρ·cp.
- HARES Ticket 001-unify-hfg-add-humidity-port.md — original requirement that equipment emit both port types for moisture mass flow tracking.
- `humidity_solver.rs:206-250` — documents the semi-implicit infiltration coupling and its EnergyPlus provenance.
