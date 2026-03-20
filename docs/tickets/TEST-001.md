# TEST-001: Thermal envelope test suite with OCHRE oracle comparison

## Status: Open

## Problem

FIX-029 (solar injected at zone node instead of exterior RC node) was not caught
by existing tests. The 94-test suite lacks:

1. No per-surface comparison against OCHRE reference data
2. No test verifies solar injects at the correct B-matrix column
3. No thermal runaway / divergence detection
4. Smoke tests have column name mismatches (NaN for HVAC comparison)
5. No energy conservation test that exercises the multi-node solar path

### Current HARES vs OCHRE comparison (BEopt 1h, May 5 noon Denver)

| Quantity | OCHRE | HARES | Gap |
|---|---|---|---|
| Indoor temp | 20.8-22.0 C | 20.2-21.6 C | close |
| ASHP Heater energy | 0.911 kWh | 0.395 kWh | -57% |
| ASHP Cooler energy | 0.050 kWh | 0.050 kWh | match |
| Ext Wall solar | 18,573 W mean | ~4,253 W total | -77% |
| Attic Roof solar | 92,800 W mean | not modelled | -100% |
| Ext Wall LWR | -7,007 W mean | unknown | ? |
| Wall heat gain indoor | -345 W mean | unknown | ? |
| Roof heat gain indoor | -171 W mean | unknown | ? |
| Non-HVAC loads | ~0.38 kWh | ~0.38 kWh | <1% |

Root cause of heater gap: HARES doesn't model enough exterior surfaces (attic
roof = 92.8 kW solar alone), so envelope heat loss is too low, so less heating
needed. The physics must match before we can claim accuracy.

## Approach: OCHRE as oracle

OCHRE output captured at `tests/fixtures/parity/beopt_smoke_1h/ochre_reference.csv`
(60 rows, 1-min resolution, 169 columns). Key oracle columns:

**Per-surface exterior (solar, LWR, surface temp):**
- `Exterior Wall Ext. Solar Gain (W)` — mean 18,573 W
- `Exterior Wall Ext. LWR Gain (W)` — mean -7,007 W
- `Exterior Wall Ext. Surface Temperature (C)` — mean 23.4 C
- `Attic Roof Ext. Solar Gain (W)` — mean 92,800 W
- `Attic Roof Ext. LWR Gain (W)` — mean -29,359 W
- `Window Ext. Solar Gain (W)` — mean 537 W
- `Door Ext. Solar Gain (W)` — mean 160 W

**Zone-level envelope heat flows:**
- `Net Sensible Heat Gain - Indoor (W)` — mean 1,737 W
- `Infiltration Heat Gain - Indoor (W)` — mean -12 W
- `Window Transmitted Solar Gain (W)` — mean 356 W
- `Wall Heat Gain - Indoor (W)` — mean -345 W
- `Roof Heat Gain - Indoor (W)` — mean -171 W
- `Floor Heat Gain - Indoor (W)` — mean -759 W
- `Radiation Heat Gain - Indoor (W)` — mean 96 W
- `Internal Heat Gain - Indoor (W)` — mean 342 W

**Zone temperatures:**
- `Temperature - Indoor (C)` — mean 21.3 C
- `Temperature - Attic (C)` — mean 14.5 C
- `Temperature - Outdoor (C)` — mean 11.7 C

Tolerance bands: where HARES uses better physics (e.g., EnergyPlus IAM vs OCHRE
simple cosine), a documented >5% deviation is acceptable. Unexplained deviations
>50% must be investigated and ticketed.

## Test plan

### Phase 1: Expose HARES envelope internals for comparison

HARES verbosity 6 already defines envelope breakdown columns (wall/roof/floor/
window heat gains, infiltration, solar). Wire these up in the thermal solver so
they're populated in the output CSV. Currently the schema exists but values may
be zero.

**Required output columns (match OCHRE names where possible):**
- `Temperature - Indoor (C)` (already at verbosity ≥2)
- `Window Transmitted Solar Gain (W)` (already defined at verbosity 6)
- `Wall Heat Gain - Indoor (W)` (defined, needs wiring)
- `Roof Heat Gain - Indoor (W)` (defined, needs wiring)
- `Floor Heat Gain - Indoor (W)` (defined, needs wiring)
- `Infiltration Heat Gain - Indoor (W)` (defined, needs wiring)
- `Radiation Heat Gain - Indoor (W)` (defined, needs wiring)

**New columns needed for per-surface oracle comparison:**
- Per-surface opaque solar gain (W) — total across all surfaces
- Per-surface LWR gain (W) — total across all surfaces
- Per-surface exterior surface temperature (C) — from state vector

### Phase 2: Oracle integration tests

#### `envelope_oracle_beopt_1h`
- **File**: `tests/parity/mod.rs` (or new `tests/envelope_oracle.rs`)
- **Setup**: Run BEopt 1h at verbosity 6, parse HARES output CSV. Load
  `tests/fixtures/parity/beopt_smoke_1h/ochre_reference.csv` as oracle.
- **Comparisons** (each is a separate named check):

| Check | OCHRE column | Tolerance | Rationale |
|---|---|---|---|
| zone_temp_indoor | Temperature - Indoor (C) | MAE < 2.0 C | within-hour transient |
| wall_heat_gain | Wall Heat Gain - Indoor (W) | mean within 100% | surfaces may differ |
| roof_heat_gain | Roof Heat Gain - Indoor (W) | mean within 100% | attic coupling |
| floor_heat_gain | Floor Heat Gain - Indoor (W) | mean within 100% | ground coupling |
| window_solar | Window Transmitted Solar Gain (W) | mean within 30% | IAM differences |
| infiltration | Infiltration Heat Gain - Indoor (W) | mean within 50% | method differences |
| total_opaque_solar | sum of Ext. Solar columns | within 5x | missing surfaces known |
| total_ext_lwr | sum of Ext. LWR columns | within 5x | missing surfaces known |
| heater_energy | ASHP Heater kWh | within 100% | depends on envelope |
| cooler_energy | ASHP Cooler kWh | within 100% | depends on envelope |

Initial tolerance bands are wide. As we fix missing surfaces and verify physics,
tighten them. The test's value is catching regressions and flagging new issues,
not passing with green immediately.

#### `envelope_oracle_resstock_1h`
- Same approach with ResStock fixture once OCHRE reference is captured.

### Phase 3: Physics unit tests (catch FIX-029 class bugs)

#### `solar_at_exterior_node_attenuates_vs_zone_node`
- **File**: `crates/hares-envelope/src/thermal_solver.rs`
- **Model**: 2-state (zone + wall), run two solvers:
  - A: solar input_index = exterior node column (correct)
  - B: solar input_index = zone sensible column (the bug)
- **Assert**: T_zone_A < T_zone_B after 60 steps
- **Why**: This is the exact property FIX-029 violated

#### `two_state_energy_conservation_with_wall_solar`
- **Model**: 2-state, solar at outer node, track dE = C1*dT1 + C2*dT2
- **Assert**: dE/dt ≈ Q_solar + Q_outdoor_cond within 2% per step
- **Why**: Energy conservation across nodes catches routing errors

#### `per_timestep_solar_rise_bounded`
- **Model**: 1R1C, 1000 W/m2 on 10 m2, absorptance 0.6
- **Assert**: single-step dT < 3x theoretical maximum
- **Why**: Any overcounting > 3x is caught immediately

#### `lwr_net_cooling_opposes_solar_heating`
- **Model**: 2-state, south wall, noon conditions, sky_temp=-10C
- **Assert**: T_zone(solar+LWR) < T_zone(solar only)
- **Why**: LWR should be net cooling; if it's heating, sign is wrong

#### `zone_temperature_bounded_24h_free_float`
- **Model**: BESTEST-like, sinusoidal outdoor + solar, no HVAC
- **Assert**: Zone temp in [-50, 80] C every step for 24h
- **Why**: Catches thermal runaway from any overcounting

### Phase 4: Smoke test hardening

#### Fix column name mismatch in `smoke_beopt_1h`
- HARES outputs `ASHP Heater Electric Power (kW)`, OCHRE expects `HVAC Heating Electric Power (kW)`
- Add column name aliases to the comparison lookup

#### Add hard assertions to `smoke_beopt_1h`
- Zone temp every timestep in [-10, 50] C
- Total electric within 10x of OCHRE reference
- ASHP Heater kWh > 0 (it IS running, just check it's reported)

### Phase 5: Additional oracle scenarios

Generate OCHRE reference data for diverse conditions:
1. **Summer cooling** — July afternoon, high solar, AC running
2. **Winter heating** — January night, no solar, furnace running
3. **Shoulder season** — March, moderate solar, mixed heating/cooling
4. **Multi-zone** — Building with attic + conditioned + basement

Each scenario: capture OCHRE CSV, commit as fixture, add oracle test.

## Priority

1. Phase 3 — unit tests (fast to write, catch FIX-029 class bugs immediately)
2. Phase 4 — smoke hardening (quick wins, catch regressions)
3. Phase 1 — expose internals (needed for meaningful oracle comparison)
4. Phase 2 — oracle integration tests (highest long-term value)
5. Phase 5 — diverse scenarios (coverage breadth)

## Known issues to investigate alongside

- **Missing exterior surfaces**: Attic roof, attic walls, doors not wired as
  exterior surfaces. This is the dominant cause of the opaque solar gap (4 kW
  vs 116 kW) and likely the heater energy gap (0.4 vs 0.9 kWh).
- **LWR magnitude**: OCHRE shows -7 kW wall LWR and -29 kW roof LWR. HARES
  reports ~18 kW in the "lwr" debug (but this includes solar). Need to separate
  solar and LWR in diagnostics.
- **Attic zone thermal coupling**: OCHRE shows attic at 14.5 C (between outdoor
  11.7 C and indoor 21.3 C). HARES zone 2 is at ~19.8 C — suggests attic
  insulation or coupling is wrong.

## Files

- `tests/fixtures/parity/beopt_smoke_1h/ochre_reference.csv` — OCHRE oracle data (committed)
- `tests/parity/mod.rs` or `tests/envelope_oracle.rs` — oracle integration tests
- `crates/hares-envelope/src/thermal_solver.rs` mod tests — physics unit tests
- `tests/regression/smoke_test.rs` — smoke hardening
- `crates/hares-io/src/output/` — envelope column wiring
