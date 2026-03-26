---
id: TARIFF-015
title: BillingPeriodSummary and TariffTelemetry PyO3 bindings
kind: implement
depends_on:
  - TARIFF-013
  - TARIFF-009
files_to_touch:
  - crates/hares-python/src/py_telemetry.rs
  - python/ochre_next/__init__.py
  - python/ochre_next/_hares.pyi
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-009.md
  - docs/tickets/TARIFF-013.md
  - crates/hares-python/src/py_telemetry.rs
verification:
  - cargo build --workspace
  - cargo test -p hares-python
  - uv run pytest tests/python/test_tariff_telemetry.py -v
  - cargo clippy --workspace
---

## Background/Context

Python users need access to tariff telemetry data — both per-step rate information (`TariffTelemetry`) and monthly billing summaries (`BillingPeriodSummary`). These are emitted by the `TariffActor` and should be accessible through the existing telemetry stream infrastructure.

## Work to Do

- [ ] Expose `BillingPeriodSummary` as PyO3 struct in `crates/hares-python/src/py_telemetry.rs`:
  - All fields as read-only Python properties:
    - `period_start: datetime`
    - `period_end: datetime`
    - `energy_charge_usd: float`
    - `demand_charge_usd: float`
    - `fixed_charge_usd: float`
    - `export_credit_usd: float`
    - `net_bill_usd: float`
    - `peak_demand_kw: float`
    - `total_import_kwh: float`
    - `total_export_kwh: float`
  - `__repr__` showing period dates and net bill
  - `to_dict() -> dict` for easy DataFrame conversion
- [ ] Expose `TariffTelemetry` as PyO3 struct:
  - All fields as read-only Python properties:
    - `period_name: str`
    - `current_rate_usd_per_kwh: float`
    - `export_rate_usd_per_kwh: float`
    - `demand_exposure_usd: float`
    - `cumulative_energy_kwh: float`
  - `__repr__`
- [ ] Add tariff telemetry types to the telemetry event enum/union so Python telemetry stream consumers can filter by type
- [ ] Export from `python/ochre_next/__init__.py`
- [ ] Add type stubs to `python/ochre_next/_hares.pyi`

## Files to Touch

- `crates/hares-python/src/py_telemetry.rs`: Add BillingPeriodSummary and TariffTelemetry PyO3 structs
- `python/ochre_next/__init__.py`: Export telemetry types
- `python/ochre_next/_hares.pyi`: Type stubs

## Measures of Success

- [ ] `BillingPeriodSummary` fields accessible as Python attributes
- [ ] `BillingPeriodSummary.to_dict()` produces dict with all fields
- [ ] `TariffTelemetry` fields accessible as Python attributes
- [ ] Both types appear in telemetry stream during simulation with tariff
- [ ] `repr()` is human-readable for both types
- [ ] Type stubs provide IDE autocomplete

## Tests Added

**tests/python/test_tariff_telemetry.py:**
- `test_billing_summary_emitted` — 35-day sim with monthly tariff → at least one BillingPeriodSummary
- `test_billing_summary_fields` — net_bill > 0, period_start is valid datetime, peak_demand > 0
- `test_billing_summary_to_dict` — to_dict() returns dict with all expected keys
- `test_tariff_telemetry_emitted_each_step` — TariffTelemetry count == step count
- `test_tariff_telemetry_period_name` — period_name matches expected TOU period
- `test_tariff_telemetry_rate_matches_tariff` — current_rate matches configured tariff rate for that hour

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-python` passes
- [ ] `uv run pytest tests/python/test_tariff_telemetry.py -v` passes
- [ ] `cargo clippy --workspace` passes
