# OCHRE parity fixture corpus

Each fixture directory is identified by climate/equipment mix and is expected to contain:

- `building.xml`
- `schedule.csv`
- `weather.epw`
- `reference_output.parquet`
- `config.toml`

Current scaffold includes 10 fixture directories with `building.xml` + `config.toml` baselines:

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

To activate a fixture for numeric parity checks, add `schedule.csv`, `weather.epw`, and
`reference_output.parquet` generated from OCHRE with matching inputs.
