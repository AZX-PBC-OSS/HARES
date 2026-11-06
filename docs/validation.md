# Validation & Verification

How HARES validates that its physics models, equipment simulations, and
whole-building results are correct.

HARES uses a layered validation strategy: individual physics functions
are checked against published reference values, equipment models are
compared against OCHRE as a reference oracle, the thermal envelope is
validated against ASHRAE 140 (BESTEST) reference bands, and conservation
laws are enforced at runtime via invariant checks.

---

## Validation Layers

| Layer | What it validates | Reference source | Location |
|-------|-------------------|------------------|----------|
| [Physics properties](#physics-reference-values) | Psychrometrics, convection, infiltration, solar | ASHRAE HOF, PsychroLib, EnergyPlus, ISA 1976 | `hares-physics/tests/physics_validation_tests.rs` |
| [Equipment parity](#ochre-parity-tests) | HVAC, water heater, DER, loads | OCHRE simulator | `hares-equipment/tests/*_parity.rs` |
| [Envelope parity](#envelope-oracle-tests) | Zone temperatures, thermal response | OCHRE simulator | `tests/*_oracle.rs` |
| [Whole-building parity](#ochre-parity-tests) | End-to-end energy, power columns | OCHRE simulator | `tests/python/test_ochre_parity.py` |
| [BESTEST](#bestest-ashrae-140) | Annual loads, peak temperatures | ASHRAE 140-2023 | `tests/bestest/` |
| [A3 acceptance](#the-normative-a3-acceptance-mode) | The normative software acceptance criteria (single-value ranges, the two dates' hourly comparisons, the minimum-case rule) | ASHRAE 140-2023 Addendum a | `tests/bestest/a3/` |
| [Conservation laws](#conservation-of-energy) | Energy balance, temperature bounds | First principles | `hares-envelope/tests/solver_energy_conservation.rs` |
| [Runtime invariants](#runtime-invariants) | Per-timestep numerical sanity | First principles | `hares-core/src/invariants.rs` |

---

## OCHRE Parity Tests

OCHRE ([Blonsky et al. 2021](https://doi.org/10.1016/j.apenergy.2021.116732),
repo at https://github.com/NREL/OCHRE) is the primary reference oracle,
vendored as a git submodule at `vendors/OCHRE/`. The pinned commit is
recorded in the "OCHRE Reference" section above. HARES runs side-by-side
simulations on identical inputs (same HPXML building description, weather
file, and schedules) and compares output columns.

### Whole-building parity

`tests/python/test_ochre_parity.py` runs both engines on a BEopt example
dwelling and compares aggregated energy (kWh) per output column.

Tolerances are set per column, informed by ASHRAE 140-2023 §7
acceptance ranges:

| Column | Tolerance | Rationale |
|--------|-----------|-----------|
| Total Electric Power | 10% | Tolerates unimplemented equipment differences |
| HVAC Heating Electric Power | 15% | ASHRAE 140 acceptance range for annual heating |
| HVAC Cooling Electric Power | 5% | Tighter — cooling is well-characterised |
| Ventilation Fan Electric Power | 2% | Schedule-driven, expects close match |
| MELs / TV / Refrigerator | 2% | Schedule-driven, expects close match |
| Indoor Lighting Electric Power | 5% | Schedule-driven |
| Exterior Lighting Electric Power | 10% | Higher tolerance for exterior controls |

When the OCHRE reference value is near zero (`< 1e-9`), the comparison
switches to an absolute bound rather than relative error.

### Equipment-level parity

Each equipment subsystem has its own parity test against OCHRE:

| Test file | What it validates |
|-----------|-------------------|
| `hares-equipment/tests/hvac_parity.rs` | HVAC fuel input, thermal output, COP |
| `hares-equipment/tests/water_heater_parity.rs` | Tank temperature evolution, draw timing |
| `hares-equipment/tests/der_parity.rs` | Battery, PV, EV equipment |
| `hares-input/tests/hpxml_parity.rs` | HPXML field extraction matches OCHRE parsing |
| `hares-input/tests/weather_parity.rs` | Weather ingestion, derived quantities |
| `hares-input/tests/schedule_parity.rs` | Schedule loading and time-indexing |
| `hares-physics/tests/solar_parity.rs` | Solar position, tilted irradiance |

### Envelope oracle tests

These compare HARES thermal envelope response against OCHRE for
controlled scenarios:

| Test file | Scenario |
|-----------|----------|
| `tests/freefloat_oracle.rs` | No equipment — envelope response only |
| `tests/conditioned_oracle.rs` | Conditioned envelope — ideal HVAC and dynamic ASHP modes |
| `tests/envelope_oracle.rs` | Zone temperature evolution |
| `tests/structural_envelope_oracle.rs` | Structural element validation |

Fixture data is stored in `tests/fixtures/` under scenario directories
(`freefloat/`, `conditioned_ideal/`, `conditioned_dynamic/`), each with
spring, summer, and winter runs at 48–72 hour durations.

---

## BESTEST (ASHRAE 140)

HARES implements ASHRAE 140-2023 standard test cases to validate the
thermal envelope solver against an internationally accepted reference.

Location: `tests/bestest/`

### Implemented cases

| Case | Description | Key metric |
|------|-------------|------------|
| 600 | Base case (high solar gain) | Annual heating and cooling loads |
| 900 | Low solar gain | Annual heating and cooling loads |
| 600FF | Free-float (no HVAC, high solar) | Peak and minimum zone temperature |
| 900FF | Free-float (no HVAC, low solar) | Peak and minimum zone temperature |
| 640 | Heating-only with supplemental resistance | Annual heating energy |
| 650 | Night ventilation (a 1703.16 m³/h fan, 1800–0700) | Annual heating and cooling loads |
| 950 | Night ventilation, high mass | Annual heating and cooling loads |

Case 650 and Case 950 both run against their bands. The fan schedule both
cases share is the `[ventilation] flow_schedule_m3_s` fixture table, expanded
to per-step schedule values and read by the thermal solver at every step;
each distinct non-zero rate also gets a discretized model variant (the
rate's conductance inside the discretization), the coupling the 950's
high-mass night purge needed to close its band.

### Reference bands

Each case has acceptance bounds from ASHRAE 140-2023 Informative Annex
B8: Table B8-1 (annual heating loads), Table B8-2 (annual sensible
cooling loads) and Table B8-5 (free-float temperatures). For example:

- Case 600 heating: 3,993–4,504 kWh/year
- Case 600 cooling: 5,432–6,162 kWh/year
- Case 600FF peak zone temp: 62.4–68.4 °C

These bands represent the range of results from validated simulation
engines (the report's example-results programs: BSIMAC, CSE, DeST,
EnergyPlus, ESP-r, TRNSYS). A result within the band
confirms the model is producing physically reasonable output.

Reference bands are defined in `tests/bestest/reference_bands.rs`.

### The edition contract

The bands pin the current edition, ANSI/ASHRAE 140-2023. The edition
chain: 140-2017's fabric cases were updated by Addendum a (approved
September 1, 2020; the document sometimes cited as "the 140-2017
errata" is this addendum, and no separate errata sheet exists), which
replaced the Annex B8 example results with the updated 52-case BESTEST
suite (ANL-20/26, DOI 10.2172/1643690); 140-2020 carried that suite
under the title 140-2017 Addendum b had introduced; 140-2023 renumbered the
standard editorially, moving the fabric tests from Section 5.2
(140-2017 and 140-2020) to Section 7. Per ASHRAE's own Standard 140
resource-files page, 140-2020 to 140-2023 changed folder and file
names, section numbering and headers only.

The pinned values are the current edition's Annex B8 example results:
the publicly fetchable Std140_TF_Results report (on ASHRAE's Section 7
accompanying-files page, data.ashrae.org/standard140; the full copy
hosted by the TRNSYS project has sha256
ebebff90bba76ee4971eb40e32fc0cc22a858abb311000ea833dc1dfdcde510e),
converted from the report's MWh to kWh. The standard's normative
acceptance ranges (Normative Annex A3, "Software Acceptance Criteria")
are their own pinned contract: the normative A3 mode below pins the
public addendum's ranges, curves and minimum-case rule, and the two
modes stay separate.

The suite's weather is Denver TMY3, station 725650
(`data/examples/USA_CO_Denver.epw`, "TMY3,725650" in the EPW header),
the weather file ASHRAE publishes for the Section 7 tests
(725650TY.CSV). A public validation report labels it "TMY2 Station
725650"; ASHRAE's own file list says TMY3, and the TMY3 file is what
the engine runs.

Reconciliation evidence: the engine's case 600 cooling drift baseline
(5,506.8 kWh, a drift lock rather than a conformance claim) sits inside
the current band (5,432–6,162) while outside the previously pinned
140-2017 band (6,137–7,964). All nine previously pinned bands differed
from the current edition's; none is shared.

### The normative A3 acceptance mode

Normative Annex A3 is the standard's own software acceptance surface,
and it runs as its own explicitly selected comparison mode beside the
B8 example bands above. Source: ANSI/ASHRAE/IBPSA Addendum a to
ANSI/ASHRAE Standard 140-2023 (approved 2025-08-29), the public
addendum PDF at
[ashrae.org](https://www.ashrae.org/file%20library/technical%20resources/standards%20and%20guidelines/standards%20addenda/140_2023_a_20250829.pdf),
sha256 `cb993847ccd4a50a10a09e27cbfc199536487490952d1438ee575e52a616e892`,
verified before transcription. The addendum revises Normative Annex A3
only; it touches neither Section 7's case definitions nor Annex B8.

The pinned contract (in `tests/bestest/a3/`):

- **Single-value ranges** (Tables A3-1 and A3-4, the addendum's
  MWh/yr converted to kWh, both bounds inclusive per A3.1): Case 600
  heating 3,750–4,980 kWh and cooling 5,000–6,830 kWh; Case 900
  heating 1,040–2,280 kWh and cooling 2,350–2,600 kWh; and the
  sensitivity rows (Case 610 - Case 600 through Case 995 - Case 985).
- **Time-series ranges** (Tables A3-2, A3-3, A3-5, A3-6): the
  February 1 and July 14 hourly zone loads compared to the
  representative curves by the Pearson correlation coefficient (curve
  shape) above 0.879 and the root mean square error (value scaling,
  kWh) below the per-case limit. The hour labels are hour-ending
  (Hour 1 is midnight to 01:00 local standard time).
- **The minimum-case rule** (Table A3-14, per A3.2): the Thermal
  Fabric Low Mass group passes when at least 30 of its 34 range cases
  are within range, the Thermal Fabric High Mass group at least 25 of
  28. A submission missing range-case results refuses: no conformance
  claim is issued from a partial run, whatever the evaluated results
  did.

The record run (`cargo test --test bestest
annex_a3_section7_group_verdicts_are_records -- --nocapture`) prints
every range case with its value and verdict. The verdicts are records,
never tuning targets: an A3 failure is a finding with numbers, and no
B8 result substitutes for an A3 one or vice versa.

The 2026-11-05 record (the corrected 60/40 gains-split case fixtures;
HARES carries annual fixtures for 600, 640, 650, 900, 920, 950, so
each group's submission is incomplete and both REFUSE conformance):

| Group | Range cases | Evaluated | Within | Disposition |
|-------|------------|-----------|--------|-------------|
| Thermal Fabric Low Mass | 34 (minimum 30) | 8 | 8 | REFUSED (26 without a result) |
| Thermal Fabric High Mass | 28 (minimum 25) | 7 | 6 | REFUSED (21 without a result) |

Every evaluated range case is WITHIN its range except one:

- **Case 900, July 14 hourly loads: OUTSIDE.** Pearson 0.9630 (above
  the 0.879 floor) but RMSE 0.1516 kWh against the limit 0.0967 kWh:
  the shape tracks, the value scaling misses by 1.57x the limit. The
  finding stands as recorded.

The within-range record, with the B8 example bands beside them:

| Range case | HARES value | A3 range | B8 example band |
|------------|-------------|----------|-----------------|
| Case 600 heating | +4,033.912 kWh | [3,750, 4,980] | [3,993, 4,504] |
| Case 600 cooling | +5,411.481 kWh | [5,000, 6,830] | [5,432, 6,162] (20.5 under the floor, drift-locked) |
| Case 640 - Case 600 heating | -1,490.222 kWh | [-2,170, -1,220] | - |
| Case 640 - Case 600 cooling | -176.067 kWh | [-560, 30] | - |
| Case 650 - Case 600 cooling | -950.963 kWh | [-1,540, -950] | - |
| Case 900 heating | +1,342.638 kWh | [1,040, 2,280] | [1,379, 1,814] (36.4 under the floor, drift-locked) |
| Case 900 cooling | +2,414.284 kWh | [2,350, 2,600] | [2,267, 2,714] |
| Case 920 - Case 900 heating | +1,875.396 kWh | [1,510, 1,920] | - |
| Case 920 - Case 900 cooling | +281.819 kWh | [80, 480] | - |
| Case 950 - Case 900 cooling | -1,784.887 kWh | [-2,000, -1,560] | - |
| Case 600 February 1 | r 0.9885, RMSE 0.4037 kWh | r > 0.879, RMSE < 0.4194 | - |
| Case 600 July 14 | r 0.9880, RMSE 0.1592 kWh | r > 0.879, RMSE < 0.1921 | - |
| Case 640 February 1 | r 0.9890, RMSE 0.3259 kWh | r > 0.879, RMSE < 0.6559 | - |
| Case 900 February 1 | r 0.9919, RMSE 0.1059 kWh | r > 0.879, RMSE < 0.3716 | - |
| Case 900 July 14 | r 0.9630, RMSE 0.1516 kWh | r > 0.879, RMSE < 0.0967 | OUTSIDE |

The Section 7 cases without annual fixtures (610, 620, 630, 660, 670,
680, 685, 695, 910, 930, 940, 960, 980, 985, 995) are the named open
surface: adding them is the path to a complete submission, and the
refusal pins stay honest until then.

### Running BESTEST

BESTEST cases are long-running (annual simulation at 5-minute timesteps)
and marked `#[ignore]`:

```bash
cargo test --release -- --ignored bestest
```

---

## Physics Reference Values

`hares-physics/tests/physics_validation_tests.rs` validates individual
physics functions against published reference values rather than OCHRE.
This catches errors independent of any simulator.

### Psychrometrics

Validated against ASHRAE Handbook of Fundamentals 2021 Ch. 1 and
PsychroLib:

- **Canonical condition** (20 °C, 50% RH): humidity ratio, enthalpy,
  wet-bulb, dew-point checked against ASHRAE HOF 2021 Table 2
- **Saturation pressure at 100 °C**: must equal 101,325 Pa ± 200 Pa
  (NIST/ISA 1976)
- **Round-trip consistency**: T,W → RH → W algebraic closure across a
  grid of 40+ conditions, with ordering constraint Tdp ≤ Twb ≤ Tdb

### Air density

- Sea-level ISA standard (15 °C, 101,325 Pa): 1.2250 ± 0.0005 kg/m³
  (ICAO Doc 7488)
- Denver altitude (1,609 m): pressure and density reductions checked
  against ISA 1976 predictions

### Convection and film coefficients

- **TARP natural convection**: h = 1.31 × ΔT^(1/3) verified at multiple
  ΔT values (EnergyPlus ERM 26.1 — Inside Surface Heat Balance: TARP Algorithm; ASHRAE HOF 2021 Ch. 4)
- **DOE-2 exterior film**: monotonically decreasing exterior film
  resistance with increasing wind speed (EnergyPlus ERM 26.1 — Outside Surface Heat Balance: DOE-2 Exterior Convection)

### Infiltration

- **ASHRAE AIM-2 wind-stack model**: monotonically increasing
  infiltration with wind speed and temperature difference (Walker &
  Wilson 1998, ASHRAE HOF Ch. 16)
- **ELA model**: buoyancy-driven component increases with ΔT

### Water mains temperature

- **Burch-Christensen 2007 model**: seasonal sinusoidal variation with
  correct peak timing (late summer) and amplitude bounds (EnergyPlus
  §11.2)

### Performance curves

- **Biquadratic evaluation**: algebraic identity verification at
  machine precision (1e-9)
- **AHRI 210/240 conditions**: HVAC capacity curves cross-checked
  against OCHRE coefficient tables

---

## Conservation of Energy

`hares-envelope/tests/solver_energy_conservation.rs` verifies that the
thermal solver conserves energy over time.

### 1R1C energy balance test

A minimal RC network (one resistance, one capacitance) is configured
with BESTEST Case 600 parameters:

- Zone starts at 20 °C, outdoor at 0 °C, no HVAC
- Runs 24 hours at 5-minute timesteps
- Verifies: change in stored thermal energy equals the time-integrated
  conducted heat loss (ΔE + ∫Q_loss·dt ≈ 0)
- **Tolerance: < 0.1% relative error** over 24 hours
- Sanity checks: zone temperature strictly decreasing, always above
  outdoor temperature

### Per-timestep invariant checks

See [Runtime Invariants](#runtime-invariants) below.

---

## Runtime Invariants

HARES checks conservation laws every timestep during simulation. These
run in every build profile, debug and release alike.

The full list of checks, tolerances, ordering contract, and error
reporting is documented in
[Invariants & Observability](invariants-and-observability.md).

Key checks:

- **Electrical balance**: solver net matches port accumulation
  (tolerance: 0.001 kW)
- **Zone temperature bounds**: all zone temperatures within [−50, 80] °C
  and finite
- **Timestep validity**: dt > 0 and finite
- **Humidity payload**: all values finite

Violations halt the simulation with a descriptive
`HaresError::InvariantViolation` error — no silent corruption.

---

## Tolerance Patterns

Different validation layers use different tolerance strategies:

| Context | Pattern | Example |
|---------|---------|---------|
| Algebraic identity | Machine precision | Polynomial evaluation: ±1e-9 |
| Published table values | Table rounding + interpolation | ASHRAE psychrometric table: ±0.1–2% |
| Model-to-model (OCHRE) | Per-column relative error | HVAC power: ±5–15% |
| Near-zero reference | Absolute bound | If OCHRE value < 1e-9, check HARES < 1e-3 |
| Conservation law | Relative residual | Energy balance: < 0.1% |
| Physical bounds | Hard limits | Zone temp ∈ [−50, 80] °C |

---

## Standards and References

| Standard / Source | How it is used |
|-------------------|----------------|
| ASHRAE 140-2023 | BESTEST reference bands for envelope validation |
| ASHRAE 140-2023 Addendum a (2025-08-29, sha256 cb993847ccd4a50a10a09e27cbfc199536487490952d1438ee575e52a616e892) | The Normative Annex A3 software acceptance criteria: single-value ranges, the February 1 and July 14 representative curves and limits, the minimum-case rule |
| ASHRAE HOF 2021 Ch. 1 | Psychrometric equations, saturation pressure |
| ASHRAE HOF 2021 Ch. 16 | Infiltration (AIM-2 wind-stack model) |
| ASHRAE HOF 2021 Ch. 4 | Natural convection (TARP h formula) |
| ASHRAE 152-2019 | Duct distribution system efficiency |
| EnergyPlus Engineering Reference | Film coefficients, water mains, performance curves, defrost, latent degradation |
| OCHRE (NREL) | Primary reference oracle for equipment and whole-building parity testing |
| PsychroLib | Cross-validation of psychrometric functions |
| ISA 1976 / ICAO Doc 7488 | Standard atmosphere for altitude corrections |
| Walker & Wilson 1998 | AIM-2 infiltration model |
| Burch & Christensen 2007 | Water mains temperature model |
| Henderson & Rengarajan (ASHRAE RP-1120) | Latent degradation model for cooling coils |
| AHRI 210/240 | HVAC rating conditions for performance curves |

### OCHRE Reference

- **Repository:** https://github.com/NREL/OCHRE
- **Vendored commit:** `58f39d8746520786c26c02ce4c08618d3053c422`
- **Publication:** Blonsky, M., Maguire, J., McKenna, K., Cutler, D., Balamurugan, S. P., & Jin, X. (2021). OCHRE: The Object-oriented, Controllable, High-resolution Residential Energy Model for Dynamic Integration Studies. *Applied Energy*, *290*, 116732. https://doi.org/10.1016/j.apenergy.2021.116732
- **Status:** Published in Applied Energy (peer-reviewed journal). The NREL technical report NREL/TP-5500-84658 (Maguire et al. 2024) is an example of a study using OCHRE.

HARES vendors OCHRE as a git submodule at `vendors/OCHRE/` and uses it as the primary reference oracle for equipment-level and whole-building parity tests. The specific commit pinned in the submodule is the auditable reference version against which all parity assertions are made. If OCHRE's algorithms change in a future commit, HARES parity tests may begin failing unless the submodule is updated and parity re-validated.

Commit hash verification: `git submodule status vendors/OCHRE`
CI check: `scripts/check-ochre-commit.sh`

---

## Running Validation Tests

```bash
# Physics reference values
cargo test -p hares-physics physics_validation

# Equipment parity against OCHRE
cargo test -p hares-equipment hvac_parity
cargo test -p hares-equipment water_heater_parity

# Envelope energy conservation
cargo test -p hares-envelope solver_energy_conservation

# Envelope oracle (vs OCHRE)
cargo test freefloat_oracle
cargo test conditioned_oracle

# Whole-building parity (requires maturin develop)
uv run pytest tests/python/test_ochre_parity.py

# BESTEST (long-running, requires --release and --ignored)
cargo test --release -- --ignored bestest

# All Rust tests
cargo test

# All Python tests
uv run pytest
```
