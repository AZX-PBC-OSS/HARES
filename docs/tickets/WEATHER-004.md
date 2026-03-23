---
id: WEATHER-004
title: PSM3 parser unit and integration tests
kind: implement
depends_on:
  - WEATHER-003
files_to_touch:
  - crates/hares-io/tests/psm3_parity.rs
  - crates/hares-io/tests/fixtures/weather/synthetic_psm3_5min.csv
references:
  - crates/hares-io/src/psm3.rs
  - vendors/OCHRE/ochre/utils/schedule.py (PSM3 loading, lines 187-203)
  - https://developer.nrel.gov/docs/solar/nsrdb/psm3-download/
verification:
  - cargo build --workspace
  - cargo clippy --workspace -- -D warnings
  - cargo test --workspace
---

## Background/Context

WEATHER-003 adds the PSM3 parser. This ticket adds test coverage including:
synthetic PSM3 file parsing, unit conversion correctness, resolution detection,
downsampling from 5-minute to hourly, and cross-format consistency.

## Work to Do

- [ ] Create a minimal synthetic PSM3 CSV fixture in
      `crates/hares-io/tests/fixtures/weather/synthetic_psm3_5min.csv`:
      - 2-line header matching real PSM3 format
      - 24 hours of synthetic data at 5-minute resolution (288 rows)
      - Include realistic temperature sinusoid, solar bell curve, constant pressure
      - Include a precipitation event

- [ ] Create `crates/hares-io/tests/psm3_parity.rs` with tests:

### Parsing tests
- [ ] `parses_synthetic_psm3_fixture`: load synthetic fixture, verify field count,
      metadata extraction (lat, lon, tz, elevation)
- [ ] `psm3_pressure_converted_to_kpa`: verify mbar→kPa conversion (÷10)
- [ ] `psm3_sky_temp_uses_clark_allen`: verify Clark-Allen formula used (no IR data);
      doc comment on test should cite "Clark & Allen (1978) clear-sky emissivity model"
- [ ] `psm3_ground_temp_uses_doe2_model`: verify DOE-2 ground temperature model used;
      doc comment should cite "DOE-2.1E Engineering Manual, Ground Temperature"

### Resolution detection tests
- [ ] `psm3_detects_5min_resolution`: verify `meta.source_step_secs == 300`
- [ ] `psm3_detects_15min_resolution`: construct 15-min fixture inline, verify 900
- [ ] `psm3_detects_hourly_resolution`: construct hourly fixture inline, verify 3600
- [ ] `psm3_rejects_irregular_resolution`: verify error on inconsistent intervals

### Resampling tests
- [ ] `psm3_5min_no_resample_at_300s`: verify no-op when target matches source
- [ ] `psm3_5min_downsample_to_hourly`: verify mean aggregation for instantaneous
      fields, solar mean preserved, precipitation sum preserved
- [ ] `psm3_downsample_preserves_daily_solar_integral`: sum of GHI over a day should
      be the same before and after downsampling (within floating-point tolerance)

### Validation tests
- [ ] `psm3_rejects_out_of_range_temperature`: temperature outside [-60, 55]°C
- [ ] `psm3_rejects_negative_solar`: GHI/DNI/DHI < 0
- [ ] `psm3_rejects_dewpoint_above_drybulb`: same physical constraint as EPW

## Files to Touch

- `crates/hares-io/tests/psm3_parity.rs`: New file — PSM3 test suite
- `crates/hares-io/tests/fixtures/weather/synthetic_psm3_5min.csv`: New synthetic
  PSM3 fixture

## Measures of Success

- [ ] All parsing tests pass with synthetic fixture
- [ ] Resolution detection is correct for all supported intervals (5/15/30/60 min)
- [ ] Downsampling preserves physical invariants (solar integral, temperature mean)
- [ ] Test doc comments cite algorithm names and sources

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
