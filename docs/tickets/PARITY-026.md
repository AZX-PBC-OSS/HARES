---
id: PARITY-026
title: Generate beopt_smoke_1h fixture from BEopt example data
kind: implement
depends_on: []
files_to_touch:
  - tests/fixtures/parity/beopt_smoke_1h/building.xml
  - tests/fixtures/parity/beopt_smoke_1h/config.toml
  - tests/fixtures/parity/beopt_smoke_1h/schedule.csv
  - tests/fixtures/parity/beopt_smoke_1h/weather.epw
  - tests/fixtures/parity/beopt_smoke_1h/reference_output.parquet
references:
  - data/examples/BEopt_example.xml
  - data/examples/BEopt_example_schedule.csv
  - data/examples/USA_CO_Denver.epw
  - tests/fixtures/parity/beopt_smoke_1h/ochre_reference.csv (existing OCHRE output)
verification:
  - cargo test -p hares-core --test parity parity_outputs
---

## Background/Context

The `beopt_smoke_1h` fixture has only `ochre_reference.csv` (OCHRE output from a
prior run). It's missing all input files. This fixture should match the BEopt
smoke test configuration (May 5, 2019, 12:00 Denver, 1-hour sim).

The existing `ochre_reference.csv` provides the reference values. Need to create
`building.xml`, `config.toml`, schedule, weather, and convert the CSV to parquet.

## Work to Do

- [ ] Copy `data/examples/BEopt_example.xml` → `building.xml`
- [ ] Copy `data/examples/BEopt_example_schedule.csv` → `schedule.csv`
- [ ] Copy `data/examples/USA_CO_Denver.epw` → `weather.epw`
- [ ] Create `config.toml` with start_time=2019-05-05T12:00:00-07:00, duration=3600, time_res=60
- [ ] Convert `ochre_reference.csv` to `reference_output.parquet` via Python

## Measures of Success

- [ ] Fixture discovered as complete by parity test
- [ ] HARES simulation succeeds for this fixture

## Verification

- [ ] `cargo test -p hares-core --test parity parity_outputs` — beopt_smoke_1h no longer skipped
