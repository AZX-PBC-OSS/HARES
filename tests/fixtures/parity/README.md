# OCHRE ballpark fixture corpus

These fixtures hold OCHRE-generated reference outputs used as a **ballpark
comparison point** for HARES, not as a correctness oracle. HARES targets
better-than-OCHRE physics per ASHRAE Handbook of Fundamentals and the
EnergyPlus Engineering Reference; deviations from OCHRE are investigated
but not necessarily corrected.

For published-tool-validated correctness testing, see:

- `tests/bestest/` — ANSI/ASHRAE 140-2017 Cases 600, 600FF, 640, 900, 900FF
  against Table B8-2 / B8-3a published bands.
- `tests/fixtures/parity/ashrae_rc_reference.json` — independently derived RC
  reference (ASHRAE HoF + EnergyPlus Eng. Ref.) for static envelope checks.

## Fixture layout

Each fixture directory is identified by climate/equipment mix and is expected
to contain:

- `building.xml`
- `schedule.csv`
- `weather.epw`
- `reference_output.parquet` — OCHRE output (ballpark reference only)
- `config.toml`

Current scaffold includes 10 fixture directories with `building.xml` +
`config.toml` baselines:

- `cz2a_gas_furnace_ac_res_wh`
- `cz4a_ashp_hpwh`
- `cz5a_minisplit_gas_wh`
- `cz6b_resistance_res_wh`
- `cz4a_pv_only`
- `cz4a_battery_only`
- `cz4a_pv_battery`
- `cz5a_ev_only`
- `cz2a_pv_ev`
- `cz6b_pv_battery_ev`

To activate a fixture for numeric ballpark checks, add `schedule.csv`,
`weather.epw`, and `reference_output.parquet` generated from OCHRE with
matching inputs.

## Tolerance bands

The driver at `tests/parity/mod.rs` enforces tolerances configured in
`tests/parity/tolerance.rs`. These bands are sized relative to ASHRAE 140
published residuals (±1 °C annual-mean zone temperature), not OCHRE-exact
parity. Short-window (≤1 h) bands are intentionally looser than the 24–72 h
conditioned-oracle suite to accommodate single-cycle phase offsets between
HARES and OCHRE.
