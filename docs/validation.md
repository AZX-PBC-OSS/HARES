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
| [BESTEST](#bestest-ashrae-140) | Annual loads, peak temperatures | ASHRAE 140-2017 | `tests/bestest/` |
| [Conservation laws](#conservation-of-energy) | Energy balance, temperature bounds | First principles | `hares-envelope/tests/solver_energy_conservation.rs` |
| [Runtime invariants](#runtime-invariants) | Per-timestep numerical sanity | First principles | `hares-core/src/invariants.rs` |

---

## OCHRE Parity Tests

OCHRE (`vendors/OCHRE/`) is the primary reference oracle. HARES runs
side-by-side simulations on identical inputs (same HPXML building
description, weather file, and schedules) and compares output columns.

### Whole-building parity

`tests/python/test_ochre_parity.py` runs both engines on a BEopt example
dwelling and compares aggregated energy (kWh) per output column.

Tolerances are set per column, informed by ASHRAE 140-2017 §5.2
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
| `hares-io/tests/hpxml_parity.rs` | HPXML field extraction matches OCHRE parsing |
| `hares-io/tests/weather_parity.rs` | Weather ingestion, derived quantities |
| `hares-io/tests/schedule_parity.rs` | Schedule loading and time-indexing |
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

HARES implements ASHRAE 140-2017 standard test cases to validate the
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

### Reference bands

Each case has acceptance bounds from ASHRAE 140-2017 Table B8-2
(loads) and Table B8-3a (temperatures). For example:

- Case 600 heating: 4,296–5,709 kWh/year
- Case 600 cooling: 6,137–7,964 kWh/year
- Case 600FF peak zone temp: 64.9–69.5 °C

These bands represent the range of results from validated simulation
engines (EnergyPlus, TRNSYS, ESP-r, etc.). A result within the band
confirms the model is producing physically reasonable output.

Reference bands are defined in `tests/bestest/reference_bands.rs`.

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
  wet-bulb, dew-point checked against ASHRAE HOF Table 2
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
  ΔT values (EnergyPlus Engineering Reference §9.4, ASHRAE HOF Ch. 25)
- **DOE-2 exterior film**: monotonically decreasing exterior film
  resistance with increasing wind speed (EnergyPlus §9.5)

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
are enabled automatically in debug builds and can be enabled in release
with `-F check_invariants`.

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
| ASHRAE 140-2017 | BESTEST reference bands for envelope validation |
| ASHRAE HOF 2021 Ch. 1 | Psychrometric equations, saturation pressure |
| ASHRAE HOF 2021 Ch. 16 | Infiltration (AIM-2 wind-stack model) |
| ASHRAE HOF 2021 Ch. 25 | Natural convection (TARP h formula) |
| ASHRAE 152-2019 | Duct distribution system efficiency |
| EnergyPlus Engineering Reference | Film coefficients, water mains, performance curves, defrost, latent degradation |
| PsychroLib | Cross-validation of psychrometric functions |
| ISA 1976 / ICAO Doc 7488 | Standard atmosphere for altitude corrections |
| Walker & Wilson 1998 | AIM-2 infiltration model |
| Burch & Christensen 2007 | Water mains temperature model |
| Henderson & Rengarajan (ASHRAE RP-1120) | Latent degradation model for cooling coils |
| AHRI 210/240 | HVAC rating conditions for performance curves |

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
