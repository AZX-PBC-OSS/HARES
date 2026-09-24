# Unify h_fg Constant and Add Humidity Port Variant

**Severity**: Critical
**Priority**: P0
**Status**: Open
**Areas**: hares-equipment, hares-types, hares-envelope, hares-physics

## Problem

Two interrelated defects create a systematic latent energy imbalance of ~1.9% across the moisture balance chain:

**Problem A — Inconsistent h_fg constant in the dehumidifier→humidity_solver path**: The latent heat of vaporization is defined at different reference temperatures across modules, producing inconsistent latent energy for the same moisture mass flow:

| Module | Value (J/kg) | Reference Temp | Source |
|--------|-------------|----------------|--------|
| `hares-physics/constants.rs:41` | 2,501,000 | 0°C (ASHRAE) | `LATENT_HEAT_VAPORISATION_0C_J_KG` |
| `hares-physics/constants.rs:45` | 2,501,000 | 0°C (ASHRAE) | `LATENT_HEAT_VAPORISATION_0C_J_KG` (J/kg companion) |
| `hares-physics/psychrometrics.rs:15` | 2,501,000 | 0°C (re-export) | `LATENT_HEAT_VAPORISATION_KJ_KG` re-exports `LATENT_HEAT_VAPORISATION_0C_KJ_KG`; this is the 0°C constant, not 20°C |
| `hares-physics/constants.rs:56` | 2,450,000 | ~20°C | `LATENT_HEAT_VAPORISATION_J_KG` (retained for documentation only; not used in any moisture balance path) |
| `hares-equipment/dehumidifier.rs:35` | 2,454,000 | ~20°C (hardcoded) | `LATENT_HEAT_VAPORIZATION_J_KG` — the sole outlier |
| `hares-envelope/thermal_solver/mod.rs:35` | 2,501,000 | 0°C (imported) | `H_FG_J_PER_KG` (infiltration latent in thermal energy balance only; not part of Problem A) |
| `hares-envelope/humidity_solver.rs:35` | 2,501,000 | 0°C (imported) | `h_fg_j_kg` default |

The true outlier is `dehumidifier.rs:35`: it uses 2,454,000 J/kg while the humidity solver uses 2,501,000 J/kg. When the dehumidifier writes `latent_gain_w = -water_removal_kg_s * 2_454_000`, and the humidity solver converts back via `delta_w = latent_gain_w * dt / (2_501_000 * rho * V)`, the round-trip introduces a ~1.9% systematic moisture mass error (47,000 / 2,501,000).

Note: `thermal_solver/mod.rs:35` (`H_FG_J_PER_KG`) is used exclusively for infiltration latent in the thermal energy balance, not in the humidity-ratio round-trip, so it is not part of Problem A.

**Problem B — No explicit humidity ratio port**: `PortContribution` (at `hares-types/src/ports.rs:51-81`) has no `Humidity` variant — only `Thermal { latent_gain_w }`. Equipment that removes moisture (AC, dehumidifier) must express moisture removal as watts of latent energy. The humidity solver at `hares-envelope/src/humidity_solver.rs:116-122` must then divide by h_fg to recover the humidity ratio change. This creates a coupling that depends on every participant using the same h_fg value. A single inconsistency (as in Problem A) silently corrupts the moisture mass balance.

## Current Behavior

1. **Dehumidifier** (`hares-equipment/src/hvac/dehumidifier.rs:35`):
   ```rust
   const LATENT_HEAT_VAPORIZATION_J_KG: f64 = 2_454_000.0;
   ```
   Used at line 185:
   ```rust
   let latent_removal_w = water_removal_kg_s * LATENT_HEAT_VAPORIZATION_J_KG;
   ```
   This constant is never imported from `hares-physics`.

2. **Thermal solver** (`hares-envelope/src/thermal_solver/mod.rs:35`):
   ```rust
   const H_FG_J_PER_KG: f64 = LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J;
   ```
   Used in `infiltration.rs:215` to compute infiltration latent:
   ```rust
   let q_latent = m_dot_lat * H_FG_J_PER_KG * (w_out - zone.humidity_ratio);
   ```

3. **Humidity solver** (`hares-envelope/src/humidity_solver.rs:24-26,35`):
   ```rust
   pub h_fg_j_kg: f64,  // default: 2_501_000
   ```
   Converts all latent_gain_w back to humidity ratio change via `humidity_ratio_increment` (line 164-175):
   ```rust
   d_w = (latent_gain_w * dt_s) / (h_fg_j_kg * rho_air * volume * moisture_buffering)
   ```

4. **Ideal HVAC cooling** (`hares-equipment/src/hvac/ideal_hvac.rs:555-559`):
   ```rust
   let sensible = capacity_w * self.shr;
   let latent = capacity_w * (1.0 - self.shr);
   ```
   Writes `latent_gain_w` to `PortContribution::Thermal`. No humidity ratio delta is written.

5. **Air conditioner** (`hares-equipment/src/hvac/air_conditioner.rs:800-806`):
   ```rust
   let fan_heat_w = fan_kw * 1000.0;
   self.hvac.write_zone_thermal_contributions(
       ports,
       -sensible_cooling_w + fan_heat_w,
       -latent_cooling_w,
       ThermalCategory::HvacCooling,
   )?;
   ```
   Writes latent as watts. No humidity ratio delta.

6. **PortContribution** (`hares-types/src/ports.rs:51-81`): No `Humidity` variant exists. Only `Thermal`, `Electrical`, `Fuel`, `Fluid`, `Custom`.

## Required Behavior

1. **Single source of truth for h_fg**: All modules must use the ASHRAE 0°C reference value (2,501,000 J/kg = 2,501 kJ/kg) imported from `hares-physics::constants::LATENT_HEAT_VAPORISATION_0C_J_KG`. The 20°C reference constant (`LATENT_HEAT_VAPORISATION_J_KG = 2,450,000`) should be retained in `constants.rs` for documentation/comparison but should NOT be used in any moisture mass balance computation.

   **Rationale**: The ASHRAE 0°C reference is the standard for moist-air enthalpy calculations (ASHRAE HoF 2021 Ch.1, Eq.30: `h = 1.006·T + W·(2501 + 1.86·T)`). Both the thermal solver and humidity solver already use 2,501,000. The dehumidifier's 2,454,000 is an isolated outlier. Using a consistent 0°C reference eliminates the systematic 1.9% error in the moisture mass balance round-trip. Temperature-dependent h_fg (EnergyPlus form: `h_fg = h_fg_0 * (1 - 0.00094815 * (T - 273.15))`) is unnecessary for the accuracy required here: at 20°C the correction is ~1.9%, which is the same magnitude as the current inconsistency. Consistent 0°C reference is superior to inconsistent temperature-dependent values.

2. **Add `Humidity` variant to `PortContribution`**:
   ```rust
   Humidity {
       zone: ZoneId,
       moisture_mass_flow_kg_s: f64,
   }
   ```
   Positive = moisture added to zone; negative = moisture removed. The port carries the raw mass-flow rate (kg/s); the humidity solver owns timestep integration using zone volume and `moisture_buffering`. This prevents equipment from embedding solver-specific quantities (zone volume, `moisture_buffering`) and keeps integration in one place.

3. **Equipment writes BOTH `Thermal { latent_gain_w }` AND `Humidity { moisture_mass_flow_kg_s }`**: All cooling/dehumidifying equipment that removes moisture must emit an explicit moisture mass-flow rate. The humidity solver accumulates `moisture_mass_flow_kg_s` per zone per timestep and integrates to a humidity ratio delta. When no `Humidity` contribution is present for a zone, the solver falls back to converting `latent_gain_w / h_fg` (the existing path).

4. **Naming**: Use existing exported constant names unchanged. Do not introduce new latent-heat constants.

5. **HumidityAccumulator**: Add a `HumidityAccumulator` to `PortSlots` (parallel to `ThermalAccumulator`) that sums `moisture_mass_flow_kg_s` per zone. The humidity solver reads this accumulator first; only if it is zero does it fall back to converting `latent_gain_w` via h_fg.

## Approach

### Phase 1: Unify h_fg constant (eliminates the 1.9% error immediately)

1. In `dehumidifier.rs`: Delete line 35 (`const LATENT_HEAT_VAPORIZATION_J_KG: f64 = 2_454_000.0;`). Add `use hares_physics::constants::LATENT_HEAT_VAPORISATION_0C_J_KG;`. Replace `LATENT_HEAT_VAPORIZATION_J_KG` with `LATENT_HEAT_VAPORISATION_0C_J_KG` at line 185.
2. Update the dehumidifier test at line 573 to import the new constant name.
3. Update `dehumidifier.rs:339` thermal port write: verify `latent_gain_w` still uses the same constant.
4. Audit all other equipment files for any remaining hardcoded h_fg values:
   - `grep -rn "2_454_000\|2_450_000\|2454\|2450" crates/`
5. Add a module-level doc comment in `hares-physics/src/constants.rs` explaining: "All moisture mass balance computations use the 0°C ASHRAE reference (2,501 kJ/kg). The 20°C reference constant is retained for informational comparison only."
6. Run the existing regression test `thermal_and_humidity_solvers_share_latent_heat_constant` in `humidity_solver.rs:484-494` — it must pass.
7. Add a new test in `dehumidifier.rs` verifying that a known water removal rate produces a latent_gain_w that round-trips correctly through the humidity solver (matching the pattern of `humidity_solver.rs:360-407`).

### Phase 2: Add Humidity port variant (eliminates the latent→moisture conversion coupling)

1. Add `Humidity` variant to `PortContribution` in `hares-types/src/ports.rs`.
2. Add `HumidityAccumulator` struct to `ports.rs` with `zone: ZoneId` and `humidity_ratio_delta_kg_kg: f64`.
3. Add `humidity: Vec<HumidityAccumulator>` field to `PortSlots`.
4. Update `PortSlots::from_declarations()` to handle `PortType::Humidity` declarations.
5. Update `PortSlots::accumulate()` to handle `PortContribution::Humidity`.
6. Update `PortSlots::zero()` to clear humidity accumulators.
7. Update the humidity solver (`humidity_solver.rs:114-161`) to read `ports.humidity` first. When a zone has a non-zero `humidity_ratio_delta_kg_kg`, use that directly. Otherwise fall back to converting `latent_gain_w` via `h_fg_j_kg` (the existing path).
8. Add `PortDeclaration::humidity(zone: ZoneId)` factory.
9. Add `PortType::Humidity` to the enum.

### Phase 3: Equipment writes humidity ratio delta

1. `dehumidifier.rs`: Compute `humidity_ratio_delta_kg_kg` from `water_removal_kg_s` and emit a `PortContribution::Humidity` alongside the existing `Thermal` contribution.
2. `ideal_hvac.rs`: When `capacity_w < 0` (cooling) and `shr < 1.0`, emit a `PortContribution::Humidity` carrying the moisture mass-flow rate (kg/s) so the solver can own integration. The Humidity port must carry mass-flow rate (kg/s), not a pre-integrated delta — the solver owns integration using zone volume and `moisture_buffering`. Do not embed the integration formula inside the equipment; doing so requires the equipment to know zone volume and `moisture_buffering`, creating tight coupling that is fragile and hard to test.
3. `air_conditioner.rs`: Similarly emit mass-flow rate from `latent_cooling_w / h_fg`.
4. Each equipment must declare a `PortDeclaration::humidity(zone)` port in addition to the thermal port.

## HPXML Wiring

The `Humidity` port variant does not require HPXML wiring — it is an internal
simulation mechanism. However, verify that the HPXML→EquipmentConfig pipeline
correctly declares the new humidity port when dehumidifier or cooling equipment
is configured. The adapter code at `python/ochre_next/adapters/` handles only
battery/PV adapters; HVAC config wiring goes through `EquipmentConfig` typed
configs in `resolve_hvac.rs`.

## Definition of Done

- [ ] `dehumidifier.rs` has NO hardcoded h_fg constant; imports from `hares-physics::constants`
- [ ] All modules use `LATENT_HEAT_VAPORISATION_0C_J_KG` (2,501,000 J/kg) for moisture mass balance
- [ ] `PortContribution::Humidity` variant exists in `hares-types/src/ports.rs` with field `moisture_mass_flow_kg_s: f64`
- [ ] `HumidityAccumulator` exists in `PortSlots` and accumulates `moisture_mass_flow_kg_s` per zone
- [ ] Humidity solver integrates accumulated `moisture_mass_flow_kg_s` to a humidity ratio delta; falls back to `latent_gain_w / h_fg` only when accumulator is zero
- [ ] `tracing::debug!` added when humidity solver uses `moisture_mass_flow_kg_s` (preferred path) vs falling back to `latent_gain_w / h_fg`
- [ ] Telemetry key `MOISTURE_MASS_FLOW_KG_S` added per zone for debugging moisture mass balance
- [ ] Dehumidifier emits both `Thermal { latent_gain_w }` and `Humidity { moisture_mass_flow_kg_s }`
- [ ] Air conditioner emits both `Thermal` and `Humidity` port contributions
- [ ] IdealHvac emits `Humidity { moisture_mass_flow_kg_s }` when cooling with SHR < 1.0
- [ ] All existing tests pass (no regressions)
- [ ] New test: dehumidifier moisture mass round-trips through humidity solver within 1e-6 kg
- [ ] New test: humidity solver prefers `moisture_mass_flow_kg_s` over `latent_gain_w` conversion
- [ ] No remaining grep hits for `2_454_000` or `2_450_000` in equipment code

## Verification

1. **Unit test — h_fg consistency**: The existing `thermal_and_humidity_solvers_share_latent_heat_constant` test at `humidity_solver.rs:484` must pass. Add an analogous test for dehumidifier: `dehumidifier_h_fg_matches_physics_constant`.

2. **Integration test — moisture mass round-trip**: Run the dehumidifier at known conditions (rated 30 L/day, 26.7°C, 60% RH). Verify:
   - `latent_removal_w = water_removal_kg_s * 2_501_000`
   - Humidity solver integrates `moisture_mass_flow_kg_s` from the `HumidityAccumulator`: `delta_w = -moisture_mass_flow_kg_s * dt_s / (rho * V * moisture_buffering)`
   - The moisture mass integrated by the solver equals the moisture mass removed by the dehumidifier (`moisture_mass_flow_kg_s * dt_s`) within 1e-6 kg.

3. **Regression test — existing infiltration latent**: Run `infiltration_latent_energy_consistent_with_moisture_mass_flow` at `thermal_solver/mod.rs:1112`. Must still pass.

4. **Audit**: `grep -rn "2_454_000\|2_450_000\|2454000\|2450000" crates/hares-equipment/` must return zero hits.

## References

- ASHRAE Handbook of Fundamentals 2021, Chapter 1, Table 2: h_fg at 0°C = 2,501 kJ/kg; at 20°C = 2,454 kJ/kg
- ASHRAE HoF 2021 Ch.1 Eq.30: Moist air enthalpy `h = 1.006·T + W·(2501 + 1.86·T)` — the 0°C reference is baked into the standard enthalpy equation
- EnergyPlus Psychrometrics.hh: `h_fg = h_fg_0 * (1 - 0.00094815 * (T - 273.15))` — temperature-dependent form; 0°C reference is `h_fg_0 = 2,501,000 J/kg`
- OCHRE uses psychrolib's 2,501 kJ/kg at 0°C for enthalpy consistency

## Related Tickets

- 002-ideal-hvac-biquadratic-fallback.md (ideal HVAC also writes latent_gain_w without moisture_mass_flow_kg_s)
- 004-semi-implicit-infiltration-latent.md (humidity solver stability at coarse timesteps)
- 005-fan-heat-diagnostic-category.md (cooling diagnostics also depend on correct latent/sensible split)

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] **Line 35 (dehumidifier.rs)** — matches exactly:
  ```rust
  const LATENT_HEAT_VAPORIZATION_J_KG: f64 = 2_454_000.0;
  ```
  Confirmed present at `crates/hares-equipment/src/hvac/dehumidifier.rs:35`.

- [x] **Line 185 (dehumidifier.rs)** — confirmed:
  ```rust
  let latent_removal_w = water_removal_kg_s * LATENT_HEAT_VAPORIZATION_J_KG;
  ```
  The local constant (2,454,000 J/kg) is used here, not any import from `hares-physics`.

- [x] **Line 573 (dehumidifier.rs test import)** — confirmed:
  ```rust
  use super::{Dehumidifier, LATENT_HEAT_VAPORIZATION_J_KG, SECONDS_PER_DAY, WATTS_PER_KILOWATT};
  ```
  The test module references the local outlier constant by name.

- [x] **Line 339 (dehumidifier.rs port write)** — confirmed at lines 332-339:
  ```rust
  ports.accumulate(&PortContribution::Thermal {
      zone: self.zone_id,
      sensible_gain_w: snapshot.sensible_gain_w,
      radiant_gain_w: 0.0,
      latent_gain_w: -snapshot.latent_removal_w,
      category: ThermalCategory::InternalGain,
  })?;
  ```
  The negative `latent_removal_w` (computed with 2,454,000) is what the humidity solver receives.

- [x] **Humidity solver (humidity_solver.rs:24-26,35)** — confirmed. Default:
  ```rust
  h_fg_j_kg: LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J, // = 2_501_000.0
  ```
  And the `humidity_ratio_increment` function at lines 163–175 uses this value in the denominator.

- [x] **Thermal solver (thermal_solver/mod.rs:35)** — confirmed:
  ```rust
  const H_FG_J_PER_KG: f64 = LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J;
  ```
  Uses the 0°C constant (2,501,000), consistent with the humidity solver.

- [x] **PortContribution (ports.rs:51-81)** — confirmed. No `Humidity` variant. Only:
  `Thermal`, `Electrical`, `Fuel`, `Fluid`, `Custom`.

- [x] **`grep -rn "2_454_000" crates/`** — returns exactly two hits, both in `dehumidifier.rs`:
  - Line 35: constant definition
  - Line 875: comment in the regression test doc-string
  No other equipment file uses 2,454,000.

- [x] **`grep -rn "2_450_000" crates/`** — one hit:
  - `hares-physics/src/constants.rs:56`: `pub const LATENT_HEAT_VAPORISATION_J_KG: f64 = 2_450_000.0;`
  This constant is not imported anywhere in the moisture balance path (confirmed by absence in grep for its name across equipment/envelope crates).

- [x] **OCHRE cross-check**: OCHRE's `Humidity.py` line 9 uses `h_vap = 2454  # kJ/kg` for its humidity update step (latent-gains-to-humidity-ratio conversion). This means OCHRE's humidity model also uses 2,454 kJ/kg, **not** 2,501 kJ/kg. However, OCHRE's psychrolib_jit.py (line 92) uses 2501 kJ/kg exclusively for enthalpy calculations. OCHRE therefore has the same internal inconsistency that HARES is trying to correct: 2,454 for moisture-removal integration, 2,501 for enthalpy. HARES has an **intentional divergence** from OCHRE's humidity model by choosing 2,501 throughout (humidity_solver uses 2,501) — but this intention is violated by the dehumidifier still using 2,454. The ticket correctly identifies this as a bug, not an intentional design choice.

- [x] **Regression test `dehumidifier_h_fg_matches_physics_constant`** (lines 879–911) — Already existed in the codebase. Had a compile error: `KG_PER_LITER_WATER` was not imported in the test module. **Fixed**: added `KG_PER_LITER_WATER` to the `use super::` import at line 573. After fix, the test **FAILS** with:
  > `dehumidifier uses h_fg = 2454000 J/kg but humidity solver uses LATENT_HEAT_VAPORISATION_0C_J_KG = 2501000 J/kg; this creates a 1.88% moisture mass balance error (ticket 001)`
  This confirms the bug is present and the test correctly diagnoses it.

- [x] **All 1189 previously-passing tests still pass** (`cargo test -p hares-equipment --lib`). The import fix introduced no regressions.

- [x] **`thermal_and_humidity_solvers_share_latent_heat_constant` (humidity_solver.rs:484)** — passes (`cargo test -p hares-envelope --lib`).

### Web-Verified Citations

**Citation 1**: ASHRAE HoF 2021 Ch. 1 — h_fg at 0°C = 2,501 kJ/kg; at 20°C = 2,454 kJ/kg

- **Source found**: Wikipedia "Water (data page)" steam table section; Engineering Toolbox water
  properties; ASHRAE Handbook Fundamentals 2017 SI references via secondary sources
- **Quoted passage** (Wikipedia Water data page, steam table, retrieved 2026-05-20):
  > At 0°C: enthalpy of vaporization = **2496.5 J/g** (≈ 2,497 kJ/kg);
  > At 20°C: enthalpy of vaporization = **2450.9 J/g** (≈ 2,451 kJ/kg)
  These are IAPWS-IF97 saturation-curve values at 0°C and 20°C respectively.
- **ASHRAE convention note**: ASHRAE Handbook of Fundamentals psychrometrics chapter uses the
  approximation **2,501 kJ/kg at 0°C** (rounded from ~2,501 kJ/kg IAPWS value at 0.01°C triple
  point) and **2,454 kJ/kg at 20°C** (interpolated/rounded). The moist air enthalpy equation
  `h = 1.006·T + W·(2501 + 1.86·T)` with the 2,501 constant is independently confirmed by
  multiple secondary sources citing ASHRAE Fundamentals Chapter 1 (e.g., Engineering Toolbox,
  energy-models.com, psychrolib overview referencing ASHRAE HoF 2017 Ch. 1).
- **Verdict**: **Confirmed** — 2,501 kJ/kg at 0°C and approximately 2,454 kJ/kg at 20°C are
  well-established ASHRAE/IAPWS standard values. The table reference (Ch. 1, Table 2) is
  consistent with known ASHRAE chapter structure but cannot be independently verified without
  paywall access to the 2021 edition. The numerical values themselves are correct.

**Citation 2**: ASHRAE HoF 2021 Ch.1 Eq.30: `h = 1.006·T + W·(2501 + 1.86·T)`

- **Source found**: Multiple secondary sources citing ASHRAE Fundamentals Chapter 1 (psychrometrics
  chapter), confirmed by: psychrolib GitHub docs (references "2017 ASHRAE Handbook — Fundamentals,
  Chapter 1"); Engineering Toolbox; academic papers (EPJ Conferences 2017 EFM paper cites the same
  form with 2501).
- **Quoted passage** (psychrolib overview.md, referencing ASHRAE HoF 2017 Ch. 1):
  > "formulae to calculate the psychrometric properties of air are widely available in the
  > literature … [references] 2017 ASHRAE Handbook — Fundamentals, Chapter 1"
  The formula `h = 1.006·T + W·(2501 + 1.86·T)` in SI units (kJ/kg) is the standard ASHRAE
  moist-air enthalpy equation. The coefficient 2,501 kJ/kg is the latent heat of vaporization
  at 0°C; the coefficient 1.86 kJ/(kg·°C) is the specific heat of water vapour (also confirmed
  by `hares-physics/constants.rs:60: CP_WATER_VAPOUR_KJ_KG_K = 1.86`).
- **Verdict**: **Confirmed**. The equation number "Eq.30" cannot be independently verified
  without paywall access, but the formula itself and its coefficients are well-established.

**Citation 3**: EnergyPlus Psychrometrics.hh — temperature-dependent h_fg formula

- **Ticket claim**: `h_fg = h_fg_0 * (1 - 0.00094815 * (T - 273.15))`
- **Source found**: EnergyPlus GitHub — `NREL/EnergyPlus` blob `3f2759c`, `src/EnergyPlus/Psychrometrics.hh`, lines 437–458. Fetched directly.
- **Actual EnergyPlus code** (quoted):
  ```cpp
  inline Real64 PsyHfgAirFnWTdb(Real64 const EP_UNUSED(w), Real64 const T)
  {
      Real64 const Temperature(max(T, 0.0));
      return (2500940.0 + 1858.95 * Temperature) - (4180.0 * Temperature);
  }
  ```
  This simplifies to: `h_fg(T) = 2500940.0 - 2321.05 * T` (J/kg, T in °C, clamped to T≥0).
- **Verdict**: **Incorrect as stated**. EnergyPlus does NOT use the multiplicative form
  `h_fg = h_fg_0 * (1 - 0.00094815 * (T - 273.15))`. It uses a **linear subtraction** of the
  difference in specific heats of vapour and liquid water: `h_fg = (h_g0 + cp_v*T) - cp_l*T`
  where `h_g0 = 2,500,940 J/kg`, `cp_v = 1858.95 J/(kg·°C)`, `cp_l = 4180.0 J/(kg·°C)`.
  The baseline constant is 2,500,940 J/kg (not 2,501,000), and the base temperature is 0°C.
  At 20°C: `h_fg = 2500940 - 2321.05 × 20 = 2,454,519 J/kg ≈ 2,454.5 kJ/kg` — consistent with
  the ASHRAE 20°C value of ~2,454 kJ/kg. The multiplicative coefficient form cited in the ticket
  appears to be a reformulation that is numerically approximately correct but **not the actual
  EnergyPlus formula**. Note: 0.00094815 × 2501000 ≈ 2371, which is not equal to 2321, so the
  multiplicative form also gives a slightly different numerical result. The spirit of the citation
  (EnergyPlus uses a temperature-dependent form with 0°C baseline near 2,501,000 J/kg) is correct.

**Citation 4**: OCHRE uses psychrolib's 2,501 kJ/kg at 0°C for enthalpy consistency

- **Source found**: `vendors/OCHRE/ochre/Models/Humidity.py` and
  `vendors/OCHRE/ochre/utils/psychrolib_jit.py` — read directly.
- **Quoted passage** (Humidity.py line 9):
  ```python
  h_vap = 2454  # kJ/kg
  ```
  (psychrolib_jit.py line 92, used for moist air enthalpy):
  ```python
  return (1.006 * t_dry_bulb + bounded_hum_ratio * (2501.0 + 1.86 * t_dry_bulb)) * 1000.0
  ```
- **Verdict**: **Partially correct**. OCHRE uses 2,501 kJ/kg **only** in its enthalpy/wet-bulb
  calculations (via psychrolib). Its humidity update model (`HumidityModel.update_humidity`) uses
  **2,454 kJ/kg** (`h_vap = 2454`) for the latent-gains-to-humidity-ratio conversion — the same
  path that HARES's humidity_solver covers. The ticket's claim that "OCHRE uses psychrolib's 2,501
  kJ/kg for enthalpy consistency" is correct for the enthalpy path but omits the fact that OCHRE's
  humidity removal model uses 2,454. HARES intentionally diverges from OCHRE's humidity model by
  using 2,501 throughout, which is the correct choice for internal consistency.

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: The core defect described in Problem A is real and confirmed by direct code
  inspection. `dehumidifier.rs:35` declares `const LATENT_HEAT_VAPORIZATION_J_KG: f64 = 2_454_000.0`
  and uses it at line 185 to compute `latent_removal_w`. The humidity solver's default `h_fg_j_kg`
  is 2,501,000 J/kg (derived from `LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J`). The round-trip
  error is confirmed by the failing regression test: **1.88% systematic moisture mass balance error**.
  The ASHRAE values (2,501 kJ/kg at 0°C, ~2,454 kJ/kg at 20°C) are independently verified via
  Wikipedia steam tables and multiple secondary sources. The EnergyPlus citation is directionally
  correct (temperature-dependent, 0°C baseline ≈ 2,501,000 J/kg) though the specific formula
  quoted is not the actual EnergyPlus code. The OCHRE citation is partially correct.
  Problem B (missing `Humidity` port variant) is a legitimate architectural deficiency — confirmed
  by reading `ports.rs:51-81`, which has no `Humidity` variant. The proposed fix is sound:
  normalising on the 0°C reference (2,501,000 J/kg) throughout the moisture balance chain, and
  adding an explicit `moisture_mass_flow_kg_s` port to eliminate the implicit h_fg coupling.

### Proposed Fix Summary

**Phase 1 (minimal, eliminates 1.88% error immediately)**:
Delete `const LATENT_HEAT_VAPORIZATION_J_KG: f64 = 2_454_000.0;` from `dehumidifier.rs:35`.
Add `use hares_physics::constants::LATENT_HEAT_VAPORISATION_0C_J_KG;`. Replace all uses of the
local constant with `LATENT_HEAT_VAPORISATION_0C_J_KG` (lines 185 and 705 in tests). Update the
`use super::` import in the test module (line 573) to remove `LATENT_HEAT_VAPORIZATION_J_KG`.
Do NOT modify any other production code.

**Phase 2 & 3** (structural, eliminates latent→moisture coupling): As described in the ticket.
No change needed to existing constants — use `LATENT_HEAT_VAPORISATION_0C_J_KG` everywhere.

### Test Written

- **File**: `crates/hares-equipment/src/hvac/dehumidifier.rs` (within existing `#[cfg(test)]`
  module, lines 879–911)
- **Status**: Test already existed in the codebase (written as part of a prior audit pass).
  It had a **compile error** (`KG_PER_LITER_WATER` not imported in the test module scope).
  **Fixed** by adding `KG_PER_LITER_WATER` to the `use super::` import at line 573.
- **What it tests**: For a dehumidifier operating at conditions that trigger moisture removal,
  recovers the implied h_fg (`latent_removal_w / water_removal_kg_s`) and asserts it equals
  `LATENT_HEAT_VAPORISATION_0C_J_KG` (2,501,000 J/kg) within 1 J/kg. Currently **FAILS** with
  the current code (implied h_fg = 2,454,000), demonstrating the bug. Will pass after Phase 1 fix.
- **No new test code was written** — the existing test was complete and correct once the import
  compile error was fixed. All 1189 previously-passing tests continue to pass.
