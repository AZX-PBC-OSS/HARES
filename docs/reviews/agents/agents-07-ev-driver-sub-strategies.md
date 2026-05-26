# EV driver sub-strategies: individual policy correctness
**Review ID**: agents-07
**Category**: agents
**Date**: 2026-05-25

## Files Reviewed
- `crates/hares-core/src/actors/ev_driver/departure.rs`
- `crates/hares-core/src/actors/ev_driver/soc_target.rs`
- `crates/hares-core/src/actors/ev_driver/time_window.rs`
- `crates/hares-core/src/actors/ev_driver/price.rs`
- `crates/hares-core/src/actors/ev_driver/solar.rs`
- `crates/hares-core/src/actors/ev_driver/soc_gate.rs`
- `crates/hares-core/src/actors/ev_driver/efficiency.rs`
- `crates/hares-core/src/actors/ev_driver/preference.rs`
- `crates/hares-core/src/actors/ev_driver/composer.rs`
- `crates/hares-core/src/actors/ev_driver/mod.rs` (actor-level usage)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/EV.py` — OCHRE reference EV implementation

## Findings

### Finding 1: SocGate lacks hysteresis — single threshold causes potential rapid cycling [Severity: high]
**Description**: The review specification requires hysteresis: "stop charging above an upper threshold and resume below a lower threshold to prevent rapid cycling." The `SocGate` implementation uses only a single `threshold` field (line 6 of `soc_gate.rs`), not separate upper/lower bounds. The `constraint` method blocks charging at `SOC >= threshold` and the `score` method recommends charging at `SOC < threshold`. This creates a tight on/off boundary at exactly the threshold value. Any small SOC fluctuation (e.g., from self-discharge, BMS estimation noise, or auxiliary load) that crosses and recrosses the threshold within consecutive timesteps will cause rapid charge/no-charge cycling.
**Code Location**: `crates/hares-core/src/actors/ev_driver/soc_gate.rs:6-8` (struct definition), lines 11-16 (`constraint`), lines 19-34 (`score`)
**Root Cause**: The `SocGate` struct defines only one `threshold` field instead of two (e.g., `upper_threshold` and `lower_threshold`). No hysteresis state is tracked.
**Impact**: For `LowSoc` and `QuickThenWait` strategies that use `SocGate` as their sole preference (`mod.rs` lines 101-114), charging oscillates on/off at the threshold boundary. This degrades equipment duty cycle and produces unrealistic charge patterns.

### Finding 2: Solar strategy does not distinguish forecast vs. actual PV [Severity: medium]
**Description**: The review asks to verify that "Solar strategy must distinguish between forecast PV (for planning) and actual available PV (for real-time dispatch) and not over-commit based on forecast." The `SolarTracking::score` method (`solar.rs` line 10-29) reads `ctx.env.electrical.pv_generation_kw` directly and uses it for immediate dispatch (setting `power_kw` to the surplus). There is no discrimination between a forecast signal (which should be used for planning/commitment) and a real-time measurement (which should be used for dispatch). If `pv_generation_kw` carries forecast values, the strategy will set a power setpoint that could exceed what the PV system actually delivers at that timestep.
**Code Location**: `crates/hares-core/src/actors/ev_driver/solar.rs:11-13`
**Root Cause**: The `ElectricalSummary` type carries only a single `pv_generation_kw` field with no semantic distinction between forecast and observed values. The `SolarTracking` preference treats it uniformly.
**Impact**: In forecast-driven simulation modes, the EV may be dispatched to charge based on PV that does not materialize, resulting in unintended grid import. In OCHRE (`EV.py`), there is no comparable solar-tracking logic — the EV simply charges at max power when plugged in, avoiding this issue.

### Finding 3: Temperature-dependent efficiency not applied to charging rate estimation [Severity: medium]
**Description**: The `DepartureDeadline::needed_charge_hours` method (`departure.rs` lines 29-42) computes the hours needed to charge from current SOC to target by dividing the energy gap by `ctx.max_charge_kw * self.efficiency`, where `self.efficiency` is a constant 0.9 (set at `mod.rs` line 277 and line 326). Meanwhile, the driving energy consumption in `mod.rs` line 416 applies `temp_efficiency_multiplier()` to miles driven to reflect temperature-dependent range loss. However, cold temperatures also degrade charging efficiency (slower acceptance rate, BMS thermal conditioning), yet the charging-time estimate does not account for this. This causes the urgency calculation to underestimate the time truly needed in cold weather, potentially leading to the vehicle being undercharged before departure.
**Code Location**: `crates/hares-core/src/actors/ev_driver/departure.rs:35` (fixed efficiency); `crates/hares-core/src/actors/ev_driver/mod.rs:416` (temperature-adjusted driving); `crates/hares-core/src/actors/ev_driver/efficiency.rs:14-36` (available temperature curve)
**Root Cause**: The temperature-dependent efficiency curve was designed and calibrated for driving energy consumption but is not reused in the charging-time estimation path.
**Impact**: In cold climates (ambient < 0°C), needed charge time may be 30-50% underestimated, causing the driver to miss their target SOC before departure. OCHRE (`EV.py` line 298) applies `EV_EFFICIENCY` (0.9) uniformly to charging power — it does not temperature-adjust either, but OCHRE does not attempt a ready-by guarantee.

### Finding 4: Time window midnight wrap implicitly assumes ≤24h span [Severity: low]
**Description**: The `TimeWindowPref::from_hours` constructor (`time_window.rs` lines 16-25) creates a window by converting hour inputs to minute-of-day. When `end_hour < start_hour` (e.g., 22:00–06:00), the midnight-wrap branch of `TimeWindow::contains()` (schedule.rs lines 144-154) correctly handles the wrap using `weekday.pred()` for the second half. This works for windows spanning exactly one midnight. If a window were specified with a duration exceeding 24 hours (e.g., 22:00 Friday to 02:00 Monday), the single `weekday.pred()` check would be insufficient because the `TimeWindow` struct stores only one `DayFilter`. This is a theoretical concern — no `ChargingStrategy` variant constructs such windows, and the `from_hours` method produces windows under 24h.
**Code Location**: `crates/hares-core/src/actors/ev_driver/time_window.rs:16-25` (constructor); `crates/hares-types/src/schedule.rs:138-155` (`TimeWindow::contains`)
**Root Cause**: The `TimeWindow` type models a single transition (either within one day or across one midnight) but does not support multi-day span semantics.
**Impact**: No current strategy generates >24h windows. The risk is only future misuse if new strategies specify multi-day windows.

### Finding 5: Departure strategy uses fixed departure minute, not log-normal uncertainty in preference layer [Severity: low]
**Description**: The review specifies that "the departure strategy uses a log-normal distribution to model driver departure time uncertainty." The `DepartureDeadline` preference (`departure.rs`) does not model departure-time uncertainty. It operates on a fixed `departure_minute` drawn from `DepartureConstraint` or `next_departure_minute`. The log-normal distribution (`DistributionKind::LogNormal` in `schedule.rs` line 182) is used at the actor level (`mod.rs` lines 394-398) via `ScheduleSource::value_at()` to sample the departure time when rolling a daily event. Once sampled, the preference treats the departure time as deterministic. This is a valid separation of concerns (stochastic draw at actor level, deterministic policy at preference level) and does not adversely affect behavior — the policy still computes urgency correctly given the drawn departure time.
**Code Location**: `crates/hares-core/src/actors/ev_driver/departure.rs:56-65` (`resolve_departure` — deterministic minute); `crates/hares-types/src/schedule.rs:182` (LogNormal definition); `crates/hares-core/src/actors/ev_driver/mod.rs:394-398` (actor-level stochastic draw)
**Root Cause**: Design choice: departure uncertainty is modeled at the daily-event-rolling stage, not in the per-step preference. The preference uses the already-sampled departure minute.
**Impact**: No operational defect. The departure preference is architecturally correct; the log-normal is applied one layer above.

### Finding 6: SocTarget does not implement ready-by computation; ready-by is handled by DepartureDeadline + Composer [Severity: low]
**Description**: The review asks to verify that "SOC target must compute a charging rate that achieves the desired SOC by the ready-by time using a linear ramp" and "verify that if ready-by has already passed, the strategy does not produce infinite or excessive power." The `SocTarget` preference (`soc_target.rs`) computes only `score = max(0, target_soc - current_soc)` and does not set a `departure_hour` or compute a required power rate. The ready-by logic is implemented by the `DepartureDeadline` preference (`departure.rs`), which sets both `departure_hour` and `target_soc` in its vote. The `ChargingComposer::emit_vote` (`composer.rs` lines 152-162) detects the `(departure_hour, target_soc)` pair and emits an `EvSetReadyBy` signal — which delegates the rate calculation to the EV equipment's BMS. The equipment is responsible for computing the linear ramp and for avoiding excessive power when the deadline has passed. This is a valid delegation pattern: the preference communicates intent and the equipment enforces physical constraints.
**Code Location**: `crates/hares-core/src/actors/ev_driver/soc_target.rs:10-21` (no departure_hour); `crates/hares-core/src/actors/ev_driver/departure.rs:68-109` (sets departure_hour); `crates/hares-core/src/actors/ev_driver/composer.rs:152-162` (emits EvSetReadyBy)
**Root Cause**: Naming suggests `SocTarget` should handle ready-by but the concern is split between `DepartureDeadline` (sets deadline) and the equipment (computes rate). The `SocTarget` preference provides a simple urgency proportional to SOC gap.
**Impact**: No functional defect as long as the EV equipment correctly handles `EvSetReadyBy`. Risk is downstream if equipment or BMS does not enforce the ready-by power cap.

### Finding 7: Price strategy percentile computation rounds to nearest index, allocates on every day-boundary check [Severity: low]
**Description**: The `compute_percentile` function (`price.rs` lines 62-70) allocates a new `Vec<f64>`, sorts it, and rounds the index to the nearest integer to find the percentile value. This is called every day boundary (when ordinal changes) via `ensure_thresholds` (lines 37-59). The allocation and sort are acceptable for typical 24-step-per-day schedules (~24 elements). However, the rounding formula `((percentile * (sorted.len() - 1) as f64).round() as usize)` (line 68) uses nearest-round for index selection rather than linear interpolation between adjacent sorted values. For small sample sizes (e.g., 24 values), this introduces granularity: the 25th percentile of 24 values rounds to index 6 (value at position 6 of 24), making the charge threshold a single discrete sample rather than a smoothed percentile.
**Code Location**: `crates/hares-core/src/actors/ev_driver/price.rs:62-70`
**Root Cause**: Nearest-neighbor index selection without interpolation produces discrete threshold values that only take on values present in the price schedule.
**Impact**: Minor — for 24-h price schedules, the percentile still identifies the cheapest subset of hours correctly. Linear interpolation would produce smoother thresholds but the practical difference is small.

### Finding 8: Composer resolves power conservatively but target_soc from highest-scored vote may not come from the highest-scored vote [Severity: low]
**Description**: In `ChargingComposer::resolve` (`composer.rs` lines 81-148), `best_target_soc` is updated only when a vote has a strictly higher score AND that vote's `target_soc` is `Some`. If the highest-scored vote has `target_soc: None` (e.g., a price-only vote with `score: 2.5`), the `best_target_soc` retains the value from a lower-scored vote that did set `target_soc` (e.g., a `SocTarget` vote with `score: 0.5`). This decouples the target_soc provenance from the highest-scored preference's intent. The alternative approach would be to always take `target_soc` from the highest-scoring vote regardless of whether `target_soc` is set, yielding `None` when the winner doesn't set it.
**Code Location**: `crates/hares-core/src/actors/ev_driver/composer.rs:90-98`
**Root Cause**: The `target_soc` resolution is gated on `vote.target_soc.is_some()`, so a high-scored non-target vote cannot clear the target.
**Impact**: The emitted `SOCTarget` may reflect a different preference than the one with the highest score. This is reasonable behavior for multi-objective charging (you want the highest urgency governing power but still want a target to charge toward), but the label in the resolved vote reflects the highest-scored preference, creating a mismatch between the label and the actual target_soc source.

### Finding 9: OCHRE reference comparison — HARES sub-strategies are substantially more sophisticated [Informational]
**Description**: The OCHRE `EV.py` reference implementation models EV charging as a simple event-based schedule: the EV charges at maximum power (`EV_MAX_POWER`) when an event is active, with a fixed efficiency of 0.9. There is no TOU pricing awareness, no SOC-based gating, no solar surplus tracking, no departure deadline, no V2G/V2H, and no composable strategy framework. The `calculate_power_and_heat` method (lines 290-313) computes `soc_max_power = (soc_max_ctrl - soc) * capacity / hours / EV_EFFICIENCY` — a linear ramp to the max SOC within the timestep, but only when receiving an external control signal. HARES's multi-preference composer architecture with scored voting and constraint overrides represents a significant behavioral modeling improvement.
**Code Location**: `vendors/OCHRE/ochre/Equipment/EV.py:290-313` (OCHRE charge calculation)
**Impact**: HARES correctly implements a richer decision model. OCHRE provides no guidance on strategy interaction or hysteresis.

## Summary
- Total findings: 9 (2 formal findings + 7 lower-severity)
- Critical: 0
- High: 1
- Medium: 2
- Low: 6

## Recommendations
1. **Add hysteresis to `SocGate`** (high priority): Add `upper_threshold` and `lower_threshold` fields (or a `hysteresis_band`). Track a boolean `charging_allowed` state that transitions at each boundary. This prevents rapid cycling for `LowSoc` and `QuickThenWait` strategies.
2. **Apply temperature-dependent efficiency to charge-time estimation**: Reuse `temp_efficiency_multiplier()` in `DepartureDeadline::needed_charge_hours` so cold-weather energy needs for charging are not underestimated.
3. **Distinguish forecast vs. actual PV in `ElectricalSummary`**: Add a `pv_forecast_kw` field (or a separate forecast domain) alongside `pv_generation_kw`. `SolarTracking` should use actual measurements for real-time dispatch and reference forecasts only for pre-commitment planning if a planning horizon is added later.
4. **Document the resolution rules in `ChargingComposer`**: Add doc comments clarifying that `target_soc` is inherited from the highest-scored vote that sets it, and that `power_kw` takes the most conservative (smallest absolute) value.
5. **Consider linear interpolation for `compute_percentile`**: For small price schedules, interpolating between adjacent sorted values would produce smoother thresholds and avoid discrete jumps.

## References / Citations
- OCHRE EV reference: `vendors/OCHRE/ochre/Equipment/EV.py` lines 290-313 (charge power calculation), line 11 (efficiency constant)
- HARES efficiency curve calibration sources listed in `crates/hares-core/src/actors/ev_driver/efficiency.rs` lines 4-8 (AAA 2019, Geotab 2020, DOE/Argonne 2024, Recurrent Auto)
- `crates/hares-types/src/schedule.rs` lines 138-155 — `TimeWindow::contains` midnight wrapping logic
- `crates/hares-types/src/schedule.rs` lines 180-182, 207-211 — `LogNormal` distribution definition and validation
