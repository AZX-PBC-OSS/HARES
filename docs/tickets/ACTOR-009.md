---
id: ACTOR-009
title: Generate OCHRE conditioned reference fixtures
kind: implement
depends_on: []
files_to_touch:
  - tests/python/generate_conditioned_oracle.py
  - tests/fixtures/conditioned_ideal/beopt_spring_72h/ochre_reference.csv
  - tests/fixtures/conditioned_ideal/beopt_spring_72h/config.json
  - tests/fixtures/conditioned_ideal/beopt_summer_48h/ochre_reference.csv
  - tests/fixtures/conditioned_ideal/beopt_summer_48h/config.json
  - tests/fixtures/conditioned_ideal/beopt_winter_48h/ochre_reference.csv
  - tests/fixtures/conditioned_ideal/beopt_winter_48h/config.json
  - tests/fixtures/conditioned_dynamic/beopt_spring_72h/ochre_reference.csv
  - tests/fixtures/conditioned_dynamic/beopt_spring_72h/config.json
  - tests/fixtures/conditioned_dynamic/beopt_summer_48h/ochre_reference.csv
  - tests/fixtures/conditioned_dynamic/beopt_summer_48h/config.json
  - tests/fixtures/conditioned_dynamic/beopt_winter_48h/ochre_reference.csv
  - tests/fixtures/conditioned_dynamic/beopt_winter_48h/config.json
references:
  - tests/python/generate_freefloat_oracle.py
  - vendors/OCHRE/ochre/Equipment/HVAC.py
  - docs/tickets/ACTOR-INDEX.md
verification:
  - python tests/python/generate_conditioned_oracle.py
---

## Background/Context

To validate HARES's IdealHvac equipment against OCHRE, we need OCHRE reference CSVs with HVAC active. This script runs OCHRE with ideal capacity HVAC, strips non-HVAC equipment, and exports zone temperatures + HVAC loads at 1-minute resolution.

## Work to Do

- [x] Create `tests/python/generate_conditioned_oracle.py` modeled on `generate_freefloat_oracle.py`
- [x] Strip non-HVAC equipment (appliances, lighting, water heater, ventilation fan) but keep HVAC heater + cooler
- [x] Use OCHRE's native setpoint schedule (from HPXML) — don't override to fixed values
- [x] Export columns: zone temps (Indoor, Attic, Outdoor), HVAC Heating/Cooling Delivered (W), HVAC setpoints, Net Sensible Heat Gain, plus all envelope diagnostics (same as freefloat)
- [x] Three scenarios: beopt_spring_72h, beopt_summer_48h, beopt_winter_48h
- [x] **Two modes per scenario:**
  - `use_ideal_capacity=True` → output to `tests/fixtures/conditioned_ideal/{scenario}/`
  - `use_ideal_capacity=False` → output to `tests/fixtures/conditioned_dynamic/{scenario}/`
- [x] Run the script and commit all fixture CSVs

## Files to Touch

- `tests/python/generate_conditioned_oracle.py`: **New** — OCHRE conditioned reference generator
- `tests/fixtures/conditioned/*/ochre_reference.csv`: **New** — generated fixture data
- `tests/fixtures/conditioned/*/config.json`: **New** — scenario metadata

## Measures of Success

- [x] Script runs without errors against OCHRE vendor directory
- [x] Indoor temperature tracks setpoint closely (within OCHRE's deadband)
- [x] HVAC loads are non-zero and physically reasonable
- [x] Three scenarios generated with expected step counts (4320, 2880, 2880)

## Verification

- [x] `uv run tests/python/generate_conditioned_oracle.py` completes
- [x] Fixture CSVs exist and have expected column count and row count

## Fixture Details

### conditioned_ideal/ (use_ideal_capacity=True)

| Scenario | Rows | Columns | HVAC Equipment |
|----------|------|---------|---------------|
| beopt_spring_72h | 4320 | 103 | ASHP Heater, ASHP Cooler |
| beopt_summer_48h | 2880 | 103 | ASHP Heater, ASHP Cooler |
| beopt_winter_48h | 2880 | 103 | ASHP Heater, ASHP Cooler |

### conditioned_dynamic/ (use_ideal_capacity=False)

| Scenario | Rows | Columns | HVAC Equipment |
|----------|------|---------|---------------|
| beopt_spring_72h | 4320 | 103 | ASHP Heater, ASHP Cooler |
| beopt_summer_48h | 2880 | 103 | ASHP Heater, ASHP Cooler |
| beopt_winter_48h | 2880 | 103 | ASHP Heater, ASHP Cooler |

### Key Columns Exported

- Temperature columns: Indoor, Attic, Outdoor, Ground
- HVAC Heating: Delivered (W), Setpoint (C), COP (-), Capacity (W), Mode
- HVAC Cooling: Delivered (W), Setpoint (C), COP (-), Capacity (W), Mode
- Envelope diagnostics: Heat gains, flow rates, surface temperatures, film coefficients
