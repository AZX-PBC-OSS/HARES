---
id: TARIFF-013
title: ElectricTariff and GasTariff PyO3 bindings with builder API
kind: implement
depends_on:
  - TARIFF-007
files_to_touch:
  - crates/hares-python/Cargo.toml
  - crates/hares-python/src/py_tariff.rs
  - crates/hares-python/src/lib.rs
  - crates/hares-python/src/py_dwelling.rs
  - python/ochre_next/__init__.py
  - python/ochre_next/_hares.pyi
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-007.md
  - crates/hares-python/src/py_dwelling.rs
verification:
  - cargo build --workspace
  - cargo test -p hares-python
  - uv run pytest tests/python/test_tariff_builder.py -v
  - cargo clippy --workspace
---

## Background/Context

Python users need to construct tariffs programmatically (builder API), load from URDB JSON files, and attach them to dwelling configuration. The Python API should feel natural with chainable builder methods and factory classmethods.

## Work to Do

- [ ] Add `hares-tariff` dependency to `crates/hares-python/Cargo.toml`
- [ ] Create `crates/hares-python/src/py_tariff.rs`
- [ ] Implement `PyElectricTariff` PyO3 class:
  - `#[classmethod] from_urdb_json(path: &str) -> PyResult<Self>` — reads file, calls `urdb::parse`
  - `#[classmethod] from_dict(dict: &Bound<PyDict>) -> PyResult<Self>` — convert dict to JSON, deserialize
  - `#[classmethod] builder() -> PyTariffBuilder` — returns builder instance
  - `__repr__` — human-readable summary (name, period count, rate range)
  - `__eq__` — structural equality
- [ ] Implement `PyTariffBuilder` PyO3 class with chainable methods:
  - `add_tou_period(name: str, windows: list, season: str) -> Self`
  - `add_energy_rate(period_name: str, season: str, rate: f64) -> Self`
  - `add_demand_rate(rate_per_kw: f64, season: str, ratchet_fraction: Option<f64>, lookback_months: Option<u8>) -> Self`
  - `set_tiered_rates(season: str, thresholds_kwh: list, rates_per_kwh: list) -> Self`
  - `set_fixed_charges(monthly_usd: f64, daily_usd: f64) -> Self`
  - `set_export_net_metering() -> Self`
  - `set_export_net_billing(tou_credits: list) -> Self`
  - `set_export_flat_rate(rate: f64) -> Self`
  - `set_minimum_charge(amount: f64) -> Self`
  - `build() -> PyElectricTariff`
  - All methods return `Self` for chaining
- [ ] Implement `PyGasTariff` PyO3 class:
  - `#[classmethod] from_dict(dict) -> Self`
  - Builder pattern similar to electric tariff
- [ ] Add `electric_tariff` and `gas_tariff` optional fields to `PyDwelling` configuration
  - `set_electric_tariff(tariff: PyElectricTariff)`
  - `set_gas_tariff(tariff: PyGasTariff)`
- [ ] Export classes from `python/ochre_next/__init__.py`
- [ ] Add type stubs to `python/ochre_next/_hares.pyi`
- [ ] Register module in `crates/hares-python/src/lib.rs`

## Files to Touch

- `crates/hares-python/Cargo.toml`: Add hares-tariff dependency
- `crates/hares-python/src/py_tariff.rs`: New PyO3 bindings for tariff types
- `crates/hares-python/src/lib.rs`: Register py_tariff module
- `crates/hares-python/src/py_dwelling.rs`: Add tariff setter methods
- `python/ochre_next/__init__.py`: Export tariff classes
- `python/ochre_next/_hares.pyi`: Type stubs

## Measures of Success

- [ ] `ElectricTariff.builder().add_tou_period(...).add_energy_rate(...).build()` works from Python
- [ ] `ElectricTariff.from_urdb_json("path/to/fixture.json")` parses correctly
- [ ] `ElectricTariff.from_dict({...})` roundtrips
- [ ] Builder methods are chainable (return self)
- [ ] Tariff can be attached to dwelling config
- [ ] `repr()` shows useful summary
- [ ] Type stubs provide IDE autocomplete

## Tests Added

**tests/python/test_tariff_builder.py:**
- `test_builder_simple_tou` — build 2-period TOU tariff, verify rates
- `test_builder_with_demand_charges` — add demand rate with ratchet
- `test_builder_with_tiers` — add tiered rates, verify threshold count
- `test_builder_chaining` — all methods return self for chaining
- `test_from_urdb_json` — load fixture, verify period count and rates
- `test_from_dict_roundtrip` — construct, to_dict, from_dict, assert equality
- `test_set_tariff_on_dwelling` — attach to PyDwelling config without error
- `test_gas_tariff_builder` — build gas tariff with seasonal tiers

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-python` passes
- [ ] `uv run pytest tests/python/test_tariff_builder.py -v` passes
- [ ] `cargo clippy --workspace` passes
