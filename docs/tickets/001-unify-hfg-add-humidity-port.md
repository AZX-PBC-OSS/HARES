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
