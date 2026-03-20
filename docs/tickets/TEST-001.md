# TEST-001: Thermal envelope integration test suite

## Status: Open

## Problem

FIX-029 (solar injected at zone node instead of exterior RC node, 33x overcounting)
was not caught by existing tests. The test suite has 94 tests but critical gaps:

1. No test verifies solar injects at the correct B-matrix column
2. No test for thermal runaway / temperature divergence detection
3. No multi-hour thermal stability tests
4. No combined solar + LWR + infiltration interaction tests
5. Smoke tests compare column names that don't match (`HVAC Heating` vs `ASHP Heater`)
6. No per-surface comparison against OCHRE reference data
7. No B-matrix structural verification

Additionally: HARES injects ~4.25 kW opaque solar vs OCHRE's ~115.8 kW for the
BEopt 1h scenario. HARES heater uses 0.395 kWh vs OCHRE 0.911 kWh (under-heating
by 57%). These gaps suggest missing exterior surfaces (attic roof alone is 92.8 kW
in OCHRE) or incorrect wiring.

## Test categories

### T1: B-matrix structural verification (unit tests in thermal_solver.rs)

#### T1.1: `solar_at_exterior_node_attenuates_vs_zone_node`
- **Model**: 2-state (zone air C=500kJ/K + wall node C=50kJ/K), R=0.5 K/W
- **Setup**: Run two solvers for 60 steps with 600 W/m2 solar, 10 m2 wall:
  - Solver A: input_index = exterior node column (correct)
  - Solver B: input_index = zone sensible column (the FIX-029 bug)
- **Assert**: T_zone_A < T_zone_B (heat must traverse wall thermal mass)
- **Catches**: FIX-029 root cause directly

#### T1.2: `b_matrix_surface_column_gain_is_at_outer_node_row`
- **Model**: 2-state from `assemble_building_rc` with one layered boundary
- **Assert**: B_d entry at wall-node row >> B_d entry at zone-air row for the
  surface injection column
- **Catches**: B-matrix construction errors in dwelling.rs

#### T1.3: `zone_sensible_columns_shifted_by_surface_count`
- **Model**: Build B_c for a building with 3 exterior layered surfaces, 1 zone
- **Assert**: Zone sensible column index = n_ext + 3 (not n_ext + 0)
- **Catches**: Index shift errors when adding per-surface columns

### T2: OCHRE reference comparison (integration tests in tests/)

#### T2.1: `per_surface_solar_gain_matches_ochre`
- **Setup**: Run BEopt 1h, extract per-surface opaque solar from solver debug
- **Reference**: OCHRE CSV breakdown per surface type:
  - Exterior walls: ~18.6 kW total
  - Attic walls: ~4.3 kW
  - Attic/pitched roof: ~92.8 kW
  - Doors: ~0.16 kW
- **Assert**: Total HARES opaque solar within 2x of OCHRE total (~115.8 kW)
- **Notes**: Requires all exterior surfaces to be wired up. Will fail until
  missing surfaces (attic roof, attic walls, doors) are connected.

#### T2.2: `heater_energy_within_50pct_of_ochre`
- **Setup**: BEopt 1h smoke test
- **Assert**: ASHP Heater kWh within 50% of OCHRE 0.911 kWh
- **Notes**: Fix smoke test column name mismatch first (`ASHP Heater` vs `HVAC Heating`)

#### T2.3: `cooler_energy_within_50pct_of_ochre`
- **Setup**: BEopt 1h smoke test
- **Assert**: ASHP Cooler kWh within 50% of OCHRE 0.050 kWh
- **Current**: Already matches at 0.050 kWh

#### T2.4: `zone_temperature_trajectory_matches_ochre`
- **Setup**: BEopt 1h, extract zone temp timeseries
- **Reference**: OCHRE zone temp for same period
- **Assert**: RMSE < 2.0 C over the 60-minute window

### T3: Thermal runaway detection (unit tests in thermal_solver.rs)

#### T3.1: `zone_temperature_stays_in_physical_bounds_48h`
- **Model**: BESTEST Case 600 with opaque solar (10 m2 south wall) + window solar
- **Setup**: Sinusoidal outdoor temp (25-35C), sinusoidal solar (0-1000 W/m2), 48h
- **Assert**: Zone temp in [-50, 80] C every timestep

#### T3.2: `free_float_temperature_rate_decays_toward_equilibrium`
- **Model**: 1R1C, constant 2000 W solar input
- **Assert**: After initial 20-step transient, |dT/dt| monotonically non-increasing
- **Catches**: Positive feedback loops / exponential divergence

#### T3.3: `per_timestep_solar_temperature_rise_bounded`
- **Model**: 1R1C, 1000 W/m2 on 10 m2 surface, absorptance 0.6
- **Assert**: Single-step dT < 3 * (absorptance * area * POA * dt / C_zone)
- **Catches**: Any overcounting > 3x

### T4: Energy conservation with solar (unit tests in thermal_solver.rs)

#### T4.1: `energy_balance_holds_with_solar_input`
- **Model**: 1R1C, constant 500 W/m2 on 5 m2 surface, absorptance 0.6
- **Assert**: Each step: |C * dT/dt - (Q_solar + Q_cond)| < 1% of max flux
- **Notes**: Extends existing `per_timestep_energy_balance_holds` which has no solar

#### T4.2: `two_state_energy_conservation_with_wall_solar`
- **Model**: 2-state (zone + wall), solar at outer node
- **Assert**: Total energy change (C_zone*dT_zone + C_wall*dT_wall) = injected energy
- **Catches**: FIX-029 — energy routing through wrong node breaks conservation

### T5: Combined physics interaction (unit tests in thermal_solver.rs)

#### T5.1: `combined_solar_lwr_infiltration_stays_bounded`
- **Model**: 1R1C with all paths active: 500 W/m2 solar, LWR (sky=-10C),
  0.5 ACH infiltration, outdoor=5C
- **Assert**: All temps in [-40, 60] C; steady state within 12 hours

#### T5.2: `lwr_net_cooling_opposes_solar_heating`
- **Model**: 2-state, south wall, noon conditions
- **Assert**: Zone temp with (solar + LWR) < zone temp with (solar only)
  because LWR is net cooling to cold sky

### T6: Smoke test hardening (tests/regression/smoke_test.rs)

#### T6.1: Fix column name mismatch
- Map `ASHP Heater Electric Power (kW)` to OCHRE's `HVAC Heating Electric Power (kW)`
- Map `ASHP Cooler Electric Power (kW)` to OCHRE's `HVAC Cooling Electric Power (kW)`

#### T6.2: `zone_temperature_in_physical_range`
- **Assert**: Every timestep indoor temp in [-10, 50] C (Denver in May)

#### T6.3: `total_electric_within_order_of_magnitude_of_ochre`
- **Assert**: 0.1 * OCHRE_total < HARES_total < 10 * OCHRE_total

### T7: Edge cases (unit tests in thermal_solver.rs)

#### T7.1: `extreme_solar_does_not_produce_nan`
- **Setup**: 2000 W/m2 on 20 m2, 100 steps
- **Assert**: All temps finite

#### T7.2: `negative_solar_does_not_heat`
- **Setup**: direct_w_m2 = -100 (bad weather data)
- **Assert**: Zone temp does not increase vs zero-solar baseline

## Priority order

1. T1.1, T1.2 (directly catch FIX-029 class bugs)
2. T3.1, T3.3 (catch thermal runaway)
3. T4.1, T4.2 (energy conservation with solar)
4. T6.1, T6.2 (smoke test hardening — quick wins)
5. T2.1, T2.2, T2.4 (OCHRE reference comparison)
6. T5.1, T5.2 (combined physics)
7. T7.1, T7.2 (edge cases)
8. T1.3, T2.3, T3.2, T6.3 (completeness)

## Known open issues

- **Missing surfaces**: Attic roof, attic walls, doors not appearing as exterior
  surfaces in HARES. This accounts for most of the 27x opaque solar gap.
- **Heater under-heating**: HARES 0.395 kWh vs OCHRE 0.911 kWh. Likely related
  to missing surfaces reducing envelope heat loss.
- **Zone temp output**: CSV column `Temperature - Indoor (C)` shows 0.0 in some
  parsers — investigate whether this is a reporting bug or actual values.

## Files to create/modify

- `crates/hares-envelope/src/thermal_solver.rs` mod tests: T1.1-T1.2, T3.1-T3.3, T4.1-T4.2, T5.1-T5.2, T7.1-T7.2
- `crates/hares-envelope/src/boundary_rc.rs` mod tests: T1.3
- `tests/regression/smoke_test.rs`: T6.1-T6.3
- `tests/parity/`: T2.1-T2.4
