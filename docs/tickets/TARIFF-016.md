---
id: TARIFF-016
title: Integration test — full TOU scenario with PV, battery, and EV
kind: implement
depends_on:
  - TARIFF-012
  - TARIFF-013
  - TARIFF-014
  - TARIFF-015
files_to_touch:
  - tests/python/test_tariff_integration.py
  - tests/fixtures/urdb/pge_e_tou_c.json
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-012.md
verification:
  - uv run pytest tests/python/test_tariff_integration.py -v
---

## Background/Context

End-to-end integration test validating the full tariff → actor → equipment → billing pipeline. Uses a real utility rate (PG&E E-TOU-C loaded from URDB fixture), a dwelling with PV, battery in `TimeOfUseOptimization` mode, and EV in `TouAware` mode. The test verifies that the battery and EV make economically rational charge/discharge decisions aligned with the tariff schedule.

This test exercises every component: tariff parsing, evaluator precomputation, actor decision-making, equipment physics, billing accumulation, and Python telemetry access.

## Work to Do

- [ ] Create `tests/python/test_tariff_integration.py`
- [ ] Set up test dwelling with:
  - PV system (e.g., 5 kW array)
  - Battery: 13.5 kWh capacity, `BmsMode.time_of_use_optimization(reserve_soc=0.2, charge_threshold_percentile=25.0, discharge_threshold_percentile=75.0)`
  - EV: 60 kWh capacity, `ChargingStrategy.tou_aware(target_soc=0.8, departure_schedule=[DepartureConstraint("weekdays", 480, 0.8)], charge_buffer_hours=2.0)`
  - Electric tariff: `ElectricTariff.from_urdb_json("tests/fixtures/urdb/pge_e_tou_c.json")`
- [ ] Run 30-day simulation at 15-minute intervals
- [ ] Collect battery power telemetry and EV power telemetry per timestep
- [ ] Classify each timestep as peak/off-peak using the tariff schedule
- [ ] Assert battery charge/discharge distribution:
  ```python
  # >70% of battery charge energy occurs during off-peak hours
  assert off_peak_charge_kwh / total_charge_kwh > 0.70
  # >70% of battery discharge energy occurs during peak hours
  assert peak_discharge_kwh / total_discharge_kwh > 0.70
  ```
- [ ] Assert EV meets departure SOC target:
  ```python
  # EV SOC >= 0.75 * target_soc at departure on all weekdays
  for departure in weekday_departures:
      assert departure.soc >= 0.75 * 0.80
  ```
- [ ] Assert EV charging concentrated in cheapest intervals:
  ```python
  # >60% of EV charge energy occurs in bottom 50% cheapest price intervals
  assert cheap_interval_charge_kwh / total_ev_charge_kwh > 0.60
  ```
- [ ] Assert billing summary plausibility:
  ```python
  summary = billing_summaries[0]
  assert 50.0 < summary.net_bill_usd < 500.0
  assert summary.peak_demand_kw > 0
  assert summary.total_import_kwh > 0
  ```
- [ ] Assert no NaN in any energy or power value across all timesteps
- [ ] Assert simulation completes without panic

## Files to Touch

- `tests/python/test_tariff_integration.py`: New integration test file
- `tests/fixtures/urdb/pge_e_tou_c.json`: URDB fixture (may already exist from TARIFF-007)

## Measures of Success

- [ ] Battery charges predominantly during off-peak (>70% of charge energy)
- [ ] Battery discharges predominantly during peak (>70% of discharge energy)
- [ ] EV meets departure SOC target on all weekdays
- [ ] EV charging concentrated in cheapest available intervals (>60%)
- [ ] BillingPeriodSummary net bill in plausible range ($50–$500)
- [ ] No panics, no NaN in any energy/power value
- [ ] Simulation completes in under 30 seconds
- [ ] TariffTelemetry emitted every step

## Tests Added

**tests/python/test_tariff_integration.py:**
- `test_battery_charges_off_peak` — >70% charge energy during off-peak
- `test_battery_discharges_peak` — >70% discharge energy during peak
- `test_ev_meets_departure_soc` — SOC ≥ 75% of target at departure
- `test_ev_charges_cheapest_intervals` — >60% charge in bottom-50% price intervals
- `test_billing_summary_plausible` — net bill $50–$500, demand > 0, energy > 0
- `test_no_nan_values` — no NaN in any power/energy column
- `test_tariff_telemetry_complete` — TariffTelemetry count matches step count

## Verification

- [ ] `uv run pytest tests/python/test_tariff_integration.py -v` passes
