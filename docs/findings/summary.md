# BESTEST 900FF investigation — consolidated findings

**Date**: 2026-04-20
**Defect**: 900FF min_zone_temp = +0.902 °C vs reference band [−6.4, −1.6] °C (+2.50 °C outlier)
**Sibling 600FF**: passes cleanly — defect is heavyweight-construction specific
**All values below are measured simulation deltas unless marked (est.)**

---

## Measured hypotheses (simulation deltas on 900FF min_zone_temp)

| # | Hypothesis | Delta | Status |
|---|---|---|---|
| **H1** | **Initial zone temp 21°C + no warmup** | **−4.53°C (21→0°C), −2.39°C (21→3°C)** | **DOMINANT ROOT CAUSE** |
| H2 | Radiant/convective split (100%→60/40) | −0.295°C | Real defect, must fix |
| H3 | Solar routing to mass nodes | −0.142°C | Current code correct |
| H4 | RC discretization (2→4 concrete nodes) | +0.029°C (wrong dir) | Ruled out |
| H5 | Zone air density (sea-level vs altitude) | <0.05°C (est.) | Defect, negligible here |
| H6 | Interior LWR | 0°C | Correct by construction |
| H7 | Interior film coefficient (TARP) | 0°C | Already implemented |

**Budget check**: H1 (~−2.4°C at warmup-appropriate init) + H2 (−0.295°C) ≈ −2.7°C. Observed outlier is +2.50°C. Combined fix puts 900FF inside band.

---

## Bugs (wrong physics, must fix)

| # | Issue | Location | 900FF impact | General impact |
|---|---|---|---|---|
| B1 | Internal gain 100% convective — ASHRAE 140 requires 60/40 | `crates/hares-envelope/src/thermal_solver/ports.rs:19` | −0.295°C (measured) | All simulations |
| B2 | Window interior LWR flux computed but never applied | `crates/hares-envelope/src/thermal_solver/longwave.rs:297-307` | est. +0.5–1.0°C | All with glazing |
| B3 | Synthetic weather sky_temp = outdoor_temp — no sky LWR | `crates/hares-core/src/dwelling/synthetic.rs:741` | None (BESTEST uses EPW) | Severe for synthetic |
| B4 | TMY3 midpoint_offset_secs = 0 — should be 1800 (30-min shift) | `crates/hares-io/src/weather/tmy3.rs:240` | Unknown | All TMY3 simulations |
| B5 | Sky temp interpolated directly instead of recomputed from interpolated inputs | `crates/hares-io/src/weather/weather.rs:465-469` | Small | Sub-hourly accuracy |
| B6 | Window exterior LWR completely skipped — 12m² glass loses ~248W to cold sky, unaccounted | `crates/hares-envelope/src/longwave_radiation.rs:42` | est. +2.0–2.5°C | All with exterior glazing |
| B7 | Mass multiplier double-counting — 7.0× on zone air PLUS explicit furniture RC nodes | `crates/hares-core/src/dwelling/conversions.rs:29` | None (BESTEST mult=1) | +28% overstated zone mass for HPXML |
| B8 | Interior solar absorptance hardcoded 0.6 — should be per-surface or 0.70 | `crates/hares-core/src/dwelling/solver_builder.rs:704` | Minor | 17% interior solar error |
| B9 | Wind direction uses ZOH instead of circular interpolation | `crates/hares-io/src/weather/weather.rs:498-502` | Minor | Incorrect at 350°→10° transitions |

---

## Shortcuts (correct direction, wrong magnitude or incomplete)

| # | Issue | Location | 900FF impact | General impact |
|---|---|---|---|---|
| S1 | Interior film coefficients frozen at init-time TARP with 12.9°C ΔT floor — should be per-timestep | `crates/hares-physics/src/film_coefficients.rs:158-160` | est. +0.3–0.5°C | Heavyweight buildings |
| S2 | RC discretization uses timestep-dependent Fourier criterion — should use diurnal diffusion length | `crates/hares-envelope/src/boundary_rc.rs:44-78` | <0.03°C (measured) | Thick slabs, non-3600s dt |
| S3 | Air density hardcoded sea-level for zone capacitance — infiltration already altitude-corrected | `crates/hares-envelope/src/boundary_rc.rs:17` | <0.05°C | 17% at Denver, 30% at 3100m |
| S4 | Free-float zones start at 21°C via `DEFAULT_SETPOINT_C` — floor concrete has ~33-day τ | `crates/hares-core/src/environment.rs:751,836` | **−4.53°C measured** | Free-float only |
| S5 | No warmup period for BESTEST — optional feature exists but fixtures don't use it | `tests/fixtures/bestest/900ff.toml` | (same cause as S4) | First 1–5 days of all runs |
| S6 | Solar absorptance default 0.60 — EnergyPlus default is 0.70 | `crates/hares-envelope/src/longwave_radiation.rs:67` | None (BESTEST sets 0.6) | 17% exterior solar error |
| S7 | Solar radiation uses ZOH while EnergyPlus uses triangular interpolation | `crates/hares-io/src/weather/weather.rs:477-491` | Minor | Step-function solar at hour boundaries |
| S8 | Ground temp interpolated monthly while EnergyPlus holds constant per month | `crates/hares-io/src/weather/epw.rs:604-659` | Minor | Smoother than EnergyPlus |

---

## Design flaws (not wrong, but misleading or incomplete)

| # | Issue | Location |
|---|---|---|
| D1 | `interior_lwr` diagnostic always reports Σq_i = 0 by conservation — misleading telemetry | `crates/hares-envelope/src/thermal_solver/longwave.rs:316` |
| D2 | Synthetic IR = 300 W/m² inconsistent with clear-sky sky_cover=0 and cold dry conditions | `crates/hares-core/src/dwelling/synthetic.rs:739-740` |
| D3 | Synthetic ground_temp = outdoor_temp — ground is warmer in winter | `crates/hares-core/src/dwelling/synthetic.rs:742` |
| D4 | Occupant radiative fraction = 0% — ASHRAE specifies 30% for seated occupants | `crates/hares-physics/src/constants.rs:136` |
| D5 | PCHIP flat extrapolation at year boundary — no cyclic wrap | `crates/hares-io/src/weather/weather.rs:704` |
| D6 | `_ => 7.0` fallback in `mass_multiplier_for_zone` — should be exhaustive match | `crates/hares-core/src/dwelling/conversions.rs:29` |

---

## Verified correct (no fix needed)

| # | Item | Location |
|---|---|---|
| ✓1 | Window emissivity 0.84 matches NFRC/ASHRAE for clear glass | `crates/hares-envelope/src/longwave_radiation.rs:54` |
| ✓2 | EPW sky temperature cascade (3-tier) matches EnergyPlus | `crates/hares-io/src/weather/epw.rs:483-502` |
| ✓3 | DOE-2 ground temperature model implementation | `crates/hares-io/src/weather/epw.rs:423-467` |
| ✓4 | PCHIP Fritsch-Carlson implementation mathematically correct | `crates/hares-io/src/weather/weather.rs:561-648` |
| ✓5 | Infiltration already uses altitude-corrected density | `crates/hares-envelope/src/thermal_solver/infiltration.rs:75` |
| ✓6 | Thermostat deadband 1.0°C within residential range | `crates/hares-core/src/actors/ideal_thermostat.rs:36` |
| ✓7 | Occupant gains 66/51.2W correct for residential (not office) | `crates/hares-physics/src/constants.rs:125,131` |
| ✓8 | Energy balance closes by construction in RC network | confirmed empirically |

---

## Fix plan (ordered by impact + correctness)

### Fix 1 (primary, dominant) — Warmup / annual-periodic initialization
**Target**: `crates/hares-envelope/src/thermal_solver/initialization.rs`, `crates/hares-core/src/environment.rs:751`
**Change**: Run warmup period (≥20 days repeated until RC state convergence, EnergyPlus §1.2 convention), OR initialize from annual-mean outdoor temp for free-float buildings.
**Predicted delta**: −2 to −4°C on 900FF; closes S4 + S5.

### Fix 2 (primary, physics correctness) — Radiant/convective split
**Target**: `crates/hares-envelope/src/thermal_solver/ports.rs`
**Change**: Add `radiant_gain_fraction` to `ThermalPort`/`PortSlots::thermal`; route radiant fraction to surface nodes by area, convective to zone air. Default 60/40 per ASHRAE 140 §5.2.4.3.
**Predicted delta**: −0.295°C on 900FF (measured); closes B1.

### Fix 3 (correctness, likely co-dominant) — Window exterior LWR
**Target**: `crates/hares-envelope/src/longwave_radiation.rs:42`
**Change**: Include window surfaces in exterior longwave 4-component model. 12m² glazing at ε≈0.84 loses ~248W to clear night sky.
**Predicted delta**: est. −2.0 to −2.5°C on 900FF (not yet measured); closes B6.

### Fix 4 (correctness) — Window interior LWR application
**Target**: `crates/hares-envelope/src/thermal_solver/longwave.rs:297-307`
**Change**: Apply computed window net LWR flux to the window energy balance.
**Predicted delta**: est. +0.5 to +1.0°C on 900FF; closes B2. (Direction opposite to B6 — partially cancel.)

### Fix 5 — Interior film coefficients per-timestep
**Target**: `crates/hares-physics/src/film_coefficients.rs:158-160`
**Change**: Recompute TARP h per timestep using current surface-air ΔT instead of freezing at init with 12.9°C floor.
**Predicted delta**: est. +0.3–0.5°C on 900FF (direction uncertain); closes S1.

### Fix 6 — Zone air density altitude correction
**Target**: `crates/hares-envelope/src/boundary_rc.rs:17,273-285` (`derive_zone_capacitances`)
**Change**: Accept site pressure; use `ρ = p / (R_air × T_K)`. Impact on BESTEST negligible; closes S3 inconsistency with infiltration.

### Fix 7–13 — Remaining bugs and shortcuts
B3 synthetic sky_temp, B4 TMY3 midpoint, B5 sky interpolation, B7 mass multiplier double-count, B8 interior solar abs hardcoded, B9 wind direction ZOH, S2 RC discretization criterion, S6 solar abs default, S7 solar ZOH, S8 ground monthly, D1–D6 design flaws.

---

## Non-additive interactions

The fixes don't simply sum. B2 (window interior LWR, +0.5-1.0°C warmer) and B6 (window exterior LWR, −2.0-2.5°C cooler) partially cancel. Fix 1 (warmup, ~−2.4°C) and Fix 2 (radiant split, −0.3°C) are roughly additive. The only way to know the final number is to implement all and measure.

---

## Empirical data — sensitivity of min_zone_temp to initial zone temperature (900FF)

| Initial zone °C | min_zone_temp_c | Delta vs 21°C baseline |
|---|---|---|
| 21 (baseline, `DEFAULT_SETPOINT_C`) | +0.9025 | — |
| 10 | +0.9025 | 0.000 |
| 5 | −0.0810 | −0.983 |
| 3 | −1.4867 | −2.389 |
| 0 | −3.6249 | −4.527 |

**Nonlinearity**: 21→10°C has no effect (concrete wall τ ≈ 3 days decays fully before Jan 9 minimum). 10→5°C has −0.98°C effect (coupled system with floor slab + infiltration has longer effective τ). 5→0°C has −3.5°C additional effect. This is consistent with the annual-periodic warmup interpretation: correct Jan 1 concrete state is much colder than 21°C after autumn cooling.

---

## Minimum-temperature heat balance (step 944, 900FF)

```
step=939  t_zone=2.564  t_out=-15.6  opaque_lwr=-1907  infiltration=-380  internal=148
step=940  t_zone=2.153  t_out=-15.6  opaque_lwr=-1826  infiltration=-370  internal=148
step=941  t_zone=1.818  t_out=-15.0  opaque_lwr=-1771  infiltration=-349  internal=148
step=942  t_zone=1.382  t_out=-15.6  opaque_lwr=-1900  infiltration=-355  internal=148
step=943  t_zone=1.013  t_out=-15.6  opaque_lwr=-1791  infiltration=-346  internal=156
step=944  t_zone=0.902  t_out=-13.3  opaque_lwr=-1552  infiltration=-289  internal=168  ← MIN
step=945  t_zone=1.525  t_out=-12.2  opaque_lwr=+929   infiltration=-263  internal=172
```

Dominant losses: opaque LWR+conduction (~−1700 to −1900W), infiltration (~−300 to −380W). Internal gain (~148–168W) covers <10% of combined loss.

---

## Sources

- ASHRAE 140-2017 §5.2.4.3, Table B8-3a — reference band and 60/40 internal gain split
- [EnergyPlus Engineering Reference §1.2 Warmup Convergence](https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/initializing-the-simulation.html)
- [NREL BESTEST-GSR](https://github.com/NREL/BESTEST-GSR) — EnergyPlus OtherEquipment Fraction_Radiant: 0.6
- [EnergyPlus Engineering Reference: Zone Internal Gains](https://bigladdersoftware.com/epx/docs/8-3/engineering-reference/zone-internal-gains.html)
- [LBNL Modelica BESTEST Cases9xx](https://simulationresearch.lbl.gov/modelica/releases/latest/help/Buildings_ThermalZones_Detailed_Validation_BESTEST_Cases9xx.html)

---

## Per-topic deep dives in this directory

- `rc_discretization.md`, `air_density.md`, `internal_gain.md`, `interior_lwr.md`, `interior_convection.md`, `initial_zone_temp_convection.md`, `nighttime_lwr.md`, `lwr.md`, `weather.md`, `energy_balance.md`, `physics_defaults.md`
- `consolidated.md` — original agent rollup (superseded by this doc)
