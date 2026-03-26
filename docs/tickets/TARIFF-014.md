---
id: TARIFF-014
title: BmsMode and ChargingStrategy PyO3 bindings
kind: implement
depends_on:
  - TARIFF-002
  - TARIFF-003
files_to_touch:
  - crates/hares-python/src/py_enums.rs
  - crates/hares-python/src/py_dwelling.rs
  - python/ochre_next/__init__.py
  - python/ochre_next/_hares.pyi
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-002.md
  - docs/tickets/TARIFF-003.md
  - crates/hares-python/src/py_enums.rs
verification:
  - cargo build --workspace
  - cargo test -p hares-python
  - uv run pytest tests/python/test_bms_modes.py -v
  - cargo clippy --workspace
---

## Background/Context

Python users need to configure battery BMS modes and EV charging strategies from the Python API. Since `BmsMode` and `ChargingStrategy` are complex enums with structured parameters (not simple string enums), they are exposed as Python classes with factory classmethods for each variant.

## Work to Do

- [ ] Add `PyBmsMode` PyO3 class to `crates/hares-python/src/py_enums.rs`:
  - Factory classmethods for each variant:
    ```python
    BmsMode.self_consumption(min_soc=0.1, max_soc=1.0, solar_only_charging=False)
    BmsMode.time_of_use_optimization(reserve_soc=0.2, charge_threshold_percentile=25.0, discharge_threshold_percentile=75.0, solar_only_charging=False)
    BmsMode.backup_reserve(target_soc=0.8, charge_from_grid=True, charge_rate_fraction=1.0)
    BmsMode.demand_response(base_mode=BmsMode.self_consumption(...), dr_discharge_rate=1.0, min_soc_during_dr=0.1)
    BmsMode.scheduled(windows=[...])
    BmsMode.storm_watch(target_soc=1.0, trigger="manual", base_mode=BmsMode.self_consumption(...))
    BmsMode.manual()
    ```
  - `__repr__` showing variant name and key parameters
  - `__eq__` for structural equality
- [ ] Add `PyGridExportRule` as Python string enum: `"solar_only"`, `"unrestricted"`, `"disabled"`
- [ ] Add `PyStormWatchTrigger` as Python string enum: `"manual"`, `"weather_signal"`
- [ ] Add `PyBmsScheduleWindow` PyO3 class:
  - Constructor: `BmsScheduleWindow(time_window=..., action=...)`
- [ ] Add `PyBmsAction` PyO3 class with factory classmethods:
  - `BmsAction.charge(rate_fraction=1.0)`
  - `BmsAction.discharge(rate_fraction=1.0)`
  - `BmsAction.idle()`
  - `BmsAction.hold(target_soc=0.5)`
- [ ] Add `PyDepartureConstraint` PyO3 class:
  - Constructor: `DepartureConstraint(day_filter="weekdays", departure_minute=480, target_soc=0.8)`
- [ ] Extend existing ChargingStrategy Python bindings (if they exist) or add new `PyChargingStrategy` with factory classmethods for V2H, V2G, SolarSurplus, and updated TouAware/PreDeparture variants
- [ ] Wire BmsMode and ChargingStrategy to battery/EV config in `py_dwelling.rs`:
  - Battery overrides: `{"bms_mode": ..., "grid_export_rule": ...}`
  - EV overrides: `{"charging_strategy": ...}`
- [ ] Export all classes from `python/ochre_next/__init__.py`
- [ ] Add type stubs to `python/ochre_next/_hares.pyi`

## Files to Touch

- `crates/hares-python/src/py_enums.rs`: Add BmsMode, GridExportRule, BmsAction, BmsScheduleWindow, DepartureConstraint, ChargingStrategy bindings
- `crates/hares-python/src/py_dwelling.rs`: Wire BmsMode/ChargingStrategy to config overrides
- `python/ochre_next/__init__.py`: Export new classes
- `python/ochre_next/_hares.pyi`: Type stubs

## Measures of Success

- [ ] All 7 BmsMode variants constructable from Python
- [ ] Nested variants work (StormWatch wrapping SelfConsumption)
- [ ] DepartureConstraint with day_filter and departure_minute works
- [ ] V2H, V2G, SolarSurplus ChargingStrategy variants work from Python
- [ ] Assignment to battery/EV config via overrides dict works
- [ ] repr() shows useful variant info
- [ ] Type stubs provide IDE autocomplete

## Tests Added

**tests/python/test_bms_modes.py:**
- `test_bms_self_consumption` — construct and verify repr
- `test_bms_tou_optimization` — construct with percentile thresholds
- `test_bms_backup_reserve` — construct with charge_from_grid flag
- `test_bms_demand_response_nested` — DemandResponse wrapping SelfConsumption
- `test_bms_scheduled_with_windows` — Scheduled with BmsScheduleWindow list
- `test_bms_storm_watch_nested` — StormWatch wrapping Manual
- `test_bms_manual` — construct manual mode
- `test_grid_export_rule_enum` — all 3 values
- `test_departure_constraint` — construct with weekday filter
- `test_charging_strategy_v2g` — V2G with price threshold
- `test_charging_strategy_v2h` — V2H with soc thresholds
- `test_charging_strategy_solar_surplus` — SolarSurplus with departures
- `test_assign_bms_to_battery_config` — set on dwelling battery overrides
- `test_assign_strategy_to_ev_config` — set on dwelling EV overrides

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-python` passes
- [ ] `uv run pytest tests/python/test_bms_modes.py -v` passes
- [ ] `cargo clippy --workspace` passes
