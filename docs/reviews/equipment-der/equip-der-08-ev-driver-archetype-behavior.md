# EV driver archetype behavior and SOC estimation divergence
**Review ID**: equip-der-08
**Category**: equipment-der
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/actors/ev_driver/ crates/hares-equipment/src/ev/mod.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/EV.py

## Findings
### Finding 1: [Severity: high]
**Description**: `current_soc()` bypasses `estimated_soc` entirely, nullifying the intended SOC estimation divergence model.

The `EvDriverActor` documentation at `crates/hares-core/src/actors/ev_driver/mod.rs:230-234` states: *"estimated_soc tracks the actor's best guess of EV SOC. It diverges from actual equipment SOC because the actor doesn't observe CC-CV taper, thermal derating, or BMS charge termination."* However, `current_soc()` at `mod.rs:432-438` reads the typed equipment core output from `env.equipment_core`, completely bypassing `estimated_soc`:

```rust
fn current_soc(&self, env: &EnvironmentState) -> f64 {
    self.equipment_id
        .and_then(|id| env.equipment_core.get(&id))
        .and_then(|co| co.state.soc)
        .map(|soc| soc.get())
        .unwrap_or(self.estimated_soc)
}
```

This function is called by `should_plug_in()` (line 444), `needs_range_anxiety_override()` (line 455), and `evaluate_charging()` (line 486). In normal simulation operation, `equipment_core` is always populated, so the driver has **perfect information** about battery SOC. The `estimated_soc` variable is dead code except as a fallback for test scenarios.

**Code Location**: `crates/hares-core/src/actors/ev_driver/mod.rs:432-438` (`current_soc`), `mod.rs:444` (`should_plug_in`), `mod.rs:455` (`needs_range_anxiety_override`), `mod.rs:486` (`evaluate_charging`)

**Root Cause**: The comment documents intended divergence behavior but `current_soc()` was implemented as a direct telemetry read, eliminating all uncertainty modeling. Either the code or the documentation is incorrect.

**Impact**: Range anxiety and all driver decisions use actual SOC, not the driver's uncertain estimate. The behavioral model lacks the documented uncertainty. If the original intent was to model driver uncertainty (e.g., the driver doesn't know CC-CV taper reduced the charge rate), this is completely absent from actual execution.

### Finding 2: [Severity: high]
**Description**: `estimated_soc` is never reconciled to actual SOC at plug-in or any point in the daily cycle.

The `estimated_soc` is decremented during driving (`mod.rs:594`) and optionally incremented for away charging (`mod.rs:606`), but there is **no code** that resets or aligns `estimated_soc` to the equipment's actual SOC when the vehicle plugs in at home. The `estimated_soc` drifts permanently across multi-day simulations:

- On each driving day, `estimated_soc` is decremented by `drive_kwh / capacity_kwh.max(0.01)` (line 594). This uses the day's sampled kWh, which is exact.
- `estimated_soc` is NOT updated when the vehicle charges at home via the equipment BMS.
- On a day when `event_day_ratio` prevents a trip and no driving occurs, `estimated_soc` is unchanged.
- After multiple days without plug-in events (e.g., `PlugInPolicy::LowSoc` where SOC stays above threshold, or `event_day_ratio < 1.0`), `estimated_soc` remains at its last driving-decremented value while the actual equipment SOC may have been fully restored by grid charging.

**Code Location**: `crates/hares-core/src/actors/ev_driver/mod.rs:594-596`, `mod.rs:639-660` (arrival logic), no reconciliation step exists.

**Root Cause**: The arrival handling at `mod.rs:639-660` transitions the phase to `HomePluggedIn` but does not reconcile `estimated_soc` to actual SOC. The design relies on `current_soc()` always reading equipment telemetry, but the `estimated_soc` field itself is left in an inconsistent state.

**Impact**: If any code path were introduced that reads `estimated_soc` directly (rather than `current_soc()`), or if `equipment_core` is unavailable, the driver would make decisions based on a stale estimate that can diverge by 10-30% or more over a week.

### Finding 3: [Severity: high]
**Description**: `needs_range_anxiety_override()` uses static `expected_daily_miles` (distribution mean) rather than the day's actual sampled distance.

At `mod.rs:450-462`, the range anxiety calculation uses `self.expected_daily_miles`, which is set once during construction to `daily_drive_miles.mean()` (line 283). The actual `DayEvent.drive_kwh` drawn each day from the stochastic distribution can be significantly higher. This creates a **systematic under-protection**:

```rust
let anxiety_kwh = (self.expected_daily_miles + self.range_anxiety_miles)
    * self.fuel_economy_kwh_per_mi
    * temp_mult;
```

**Example failure scenario**: The `LongCommuterL2` archetype has `daily_drive_miles_mean: 75.0`, `daily_drive_miles_stddev: 20.0`. If a day draws ~115 miles (2 sigma above mean) and the vehicle is a Chevy Bolt EV (65 kWh, 0.251 kWh/mi):
- `expected_daily_miles` = 75 (static mean)
- `range_anxiety_miles` = 20 (default)
- `anxiety_soc` = (75+20) * 0.251 * 1.11 / 65 = 0.407
- **Actual needed**: 115 * 0.251 * 1.11 = 32.0 kWh = 0.493 SOC

The driver would only override charging when SOC < 0.407, but needs 0.493 to complete the day's trip. If SOC is at 0.45 (above anxiety threshold but below actual need), range anxiety does NOT fire and the driver risks being stranded.

**Code Location**: `crates/hares-core/src/actors/ev_driver/mod.rs:450-462`

**Root Cause**: The computed `expected_daily_miles` is a distribution-level statistic, not a day-specific value. The day's actual sampled miles are available in `todays_event.drive_kwh / fuel_economy_kwh_per_mi` but are never used in the anxiety calculation.

**Impact**: The driver can be stranded on days where the stochastic draw produces above-average mileage. For archetypes with high daily mileage variance (LongCommuterL2 CV=0.27, HeavyUseSuv CV=0.33, WeekendWarrior CV=0.40), this is a realistic failure mode.

### Finding 4: [Severity: medium]
**Description**: Distribution family mismatch -- archetypes use Gaussian, not LogNormal, for daily distance and behavioral parameters.

The review specification and `DistributionKind` docs (`crates/hares-types/src/schedule.rs:180-182`) describe LogNormal distributions for EV behavioral sampling. However, all 12 archetype presets in `crates/hares-equipment/src/ev/catalog.rs:538-816` use `DistributionKind::Gaussian` with `mean`/`std_dev` parameters via `build_miles_schedule()` (line 436-496), `build_departure_schedule()` (line 499-513), and `build_duration_schedule()` (line 516-530).

Gaussian distributions on positive-only quantities (miles, minutes) can produce negative draws. While clamping at `clamp_min` prevents invalid outputs, it distorts the effective distribution statistics. For daily miles with CV > 0.3 (e.g., RetireeL1 mean=10, stddev=5 gives CV=0.5), ~2.3% of draws are below zero and get clamped, raising the effective mean above the configured value.

LogNormal is standard in EV modeling literature (EVI-Pro, NREL EVI-Equity) because trip distance distributions are right-skewed and strictly positive.

**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:479-495` (miles), `crates/hares-equipment/src/ev/catalog.rs:499-513` (departure), `crates/hares-equipment/src/ev/catalog.rs:516-530` (duration)

**Root Cause**: The `DistributionKind::LogNormal` variant exists and is validated in `schedule.rs:207-213`, but was not used in the archetype catalog.

**Impact**: The daily mileage distribution has incorrect lower-tail behavior (can produce zero/negative clamped values for small-mileage archetypes) and incorrect upper-tail behavior (Gaussian tails are symmetric, whereas real trip distances have positive skew). The departure time distributions (mean 6-10 AM, std_dev 30-90 min) are largely unaffected by the choice of distribution family because they're well-centered within [0, 1440].

### Finding 5: [Severity: medium]
**Description**: `arrival_fuzz_minutes` and `departure_fuzz_minutes` are defined on all 12 archetype presets but never consumed by any code path.

Each `ArchetypePreset` in `catalog.rs:408-409` defines `arrival_fuzz_minutes` and `departure_fuzz_minutes` fields with values ranging 30-45 minutes. These are validated in tests (`catalog.rs:948-956`). However, no code reads these fields:

- `EvDriverActor::new()` (mod.rs:256-318) does not accept fuzz parameters.
- `actor_registry.rs:179-196` does not read them from the preset.
- `maybe_roll_daily_event()` (mod.rs:379-423) does not apply fuzzing to departure or arrival times.

The actual fuzzing uses separate Gaussian-distributed `departure_schedule` and `duration_schedule`, making these preset fields redundant dead data.

**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:408-409`, checked against all call sites in `crates/hares-core/src/actors/ev_driver/mod.rs` and `crates/hares-core/src/actor_registry.rs`

**Root Cause**: The preset data structure was likely designed before the `ScheduleSource::Stochastic` approach was finalized. The fuzz fields became vestigial when individual stochastic schedules replaced them.

**Impact**: Low direct impact, but indicates code rot and may confuse future maintainers who expect these values to affect simulation behavior.

### Finding 6: [Severity: medium]
**Description**: `needs_range_anxiety_override()` cannot fire on non-driving days because `evaluate_charging()` is unreachable when `todays_event` is `None`.

At `mod.rs:524-530`, when `todays_event` is `None` (a non-driving day), the `decide()` method returns early before reaching `evaluate_charging()`:

```rust
let event = match self.todays_event {
    Some(ev) => ev,
    None => {
        self.populate_telemetry(before_out, out);
        return;
    }
};
```

This means on a non-driving day, even if SOC is critically low (below the anxiety threshold), the range anxiety override can never be activated because `evaluate_charging()` (which contains the anxiety check at line 473) is never called. The driver might skip a day's trip but still need to charge for the next day's trip -- this protection is absent.

The test at `mod.rs:1839-1865` verifies that `needs_range_anxiety_override()` returns `true` on a non-driving day, but the integration point at `decide()` prevents it from being acted upon.

**Code Location**: `crates/hares-core/src/actors/ev_driver/mod.rs:524-530` (early return), `mod.rs:470-484` (anxiety check inside `evaluate_charging`)

**Root Cause**: The range anxiety check is gated behind the `todays_event.is_some()` branch, which assumes the driver only needs to charge on driving days.

**Impact**: On non-driving days, a low-SOC vehicle will not receive the range anxiety override. If the vehicle is left unplugged at low SOC and a driving day follows, the driver could be stranded. This is partially mitigated by `PlugInPolicy::Always` (which always plugs in) but is a concern for `PlugInPolicy::LowSoc` archetypes (WfhOccasional, WeekendWarrior).

### Finding 7: [Severity: medium]
**Description**: Departure time `ScheduleSource::value_at()` is called with the `EnvironmentState` but the `Stochastic` variant draws a new random value every call, not once per day.

In `maybe_roll_daily_event()` at `mod.rs:394-404`, departure time, duration, and miles are sampled via `ScheduleSource::value_at(env)`. For `ScheduleSource::Stochastic`, each call draws a new random sample (see `schedule.rs:648-654`). This is correct because `maybe_roll_daily_event` is called only once per day (guarded by `current_day_ordinal`). However, there is a subtle concern: `departure_time.value_at(env)` at line 396 is called with the current hour/minute as context, but for `Stochastic` sources, the draw does not depend on the env at all -- it's a pure RNG draw. This means the first timestep of each day determines the departure time, and if the timestep is 1 minute, the departure time is sampled at midnight. This is correct behavior, but the `env` parameter is misleading since the stochastic draw ignores it.

**Code Location**: `crates/hares-core/src/actors/ev_driver/mod.rs:394-404`, `crates/hares-types/src/schedule.rs:641-654`

### Finding 8: [Severity: low]
**Description**: `build_preferences()` uses hardcoded charging efficiency of 0.9 (line 277) instead of the equipment's configured efficiency.

At `mod.rs:276-280`, the `build_preferences()` call passes `0.9` for efficiency rather than reading from `self.charging_efficiency` (which defaults to `DEFAULT_EFFICIENCY` and can be configured per equipment):

```rust
let prefs = build_preferences(
    &strategy,
    max_charge_kw,
    0.9, // charging efficiency for energy calc
    None,
    24,
);
```

This hardcoded value is used by `DepartureDeadline.needed_charge_hours()` to compute urgency timing. If the equipment config specifies a different charging efficiency (e.g., 0.85), the urgency calculation will be 5-6% off, potentially causing the driver to charge too late or too early relative to the actual physics.

**Code Location**: `crates/hares-core/src/actors/ev_driver/mod.rs:276-280`

### Finding 9: [Severity: low]
**Description**: Trip duration `clamp_max` of 1200 minutes (20 hours) exceeds the physical day boundary, and arrival wrapping to midnight is handled by clamping.

At `catalog.rs:527`, `build_duration_schedule` sets `clamp_max: Some(1200.0)`. Combined with the departure time at `maybe_roll_daily_event:406`: `let arrival = (departure_minute as u32 + duration_minutes as u32).min(1439) as u16;`, any departure+duration exceeding 1439 is clamped to 1439 (23:59). For late-shift archetypes (e.g., ShiftWorker with departure_mean=360=06:00 and duration_mean=540=9h, arrival would be 09:00+09:00=15:00=900 which is fine), this is not an issue. But for the WeekendWarrior archetype (departure_mean=540=09:00, duration_mean=480=8h, std_dev=90m), a 2-sigma draw of 540+180=660 would give departure 600+660=1260 which is fine. The boundary is unlikely to be hit for any realistic archetype, making this a low-risk finding.

**Code Location**: `crates/hares-core/src/actors/ev_driver/mod.rs:406`

### Finding 10: [Severity: low]
**Description**: OCHRE comparison: HARES uses separable independent Gaussian draws vs. OCHRE's data-driven joint PDF.

OCHRE's `EV.py` (lines 115-200) generates events from a joint probability density function (PDF) stored in CSV files derived from EVI-Pro data. The joint PDF correlates arrival time, arrival SOC, and parking duration. OCHRE also conditions events on temperature and weekday vs. weekend, and uses capacity-dependent `event_day_ratio`.

HARES' approach separates these into independent stochastic draws:
- Departure time: independent Gaussian
- Trip duration: independent Gaussian
- Daily miles: independent Gaussian

This means HARES does not capture correlations between commute distance and departure time (longer commutes tend to depart earlier), between distance and arrival SOC, or temperature-dependent event frequency. OCHRE's event grouping by temperature bins (line 143-145) provides richer behavior for thermal effects on charging. However, HARES' approach is more configurable and requires no external data files.

**Comparison Location**: `crates/hares-core/src/actors/ev_driver/mod.rs:379-423` vs. `vendors/OCHRE/ochre/Equipment/EV.py:115-183`

## Summary
- Total findings: 10
- Critical: 0
- High: 3
- Medium: 4
- Low: 3

## Recommendations
1. **Fix SOC estimation divergence (Finding 1)**: Either (a) commit to the divergence model by using `estimated_soc` in `should_plug_in()`, `needs_range_anxiety_override()`, and `evaluate_charging()` and reconciling at plug-in, or (b) remove `estimated_soc` as dead code and update the documentation to reflect that the driver has perfect information. Option (a) better captures the intended range-anxiety behavior.

2. **Reconcile `estimated_soc` at plug-in (Finding 2)**: Add a reconciliation step in the arrival handling code when transitioning to `HomePluggedIn` phase that sets `estimated_soc = current_soc(env)`, aligning the driver's belief with equipment reality at the start of each home charging session.

3. **Fix range anxiety to use day-specific miles (Finding 3)**: Change `needs_range_anxiety_override()` to compute the anxiety threshold using the day's actual `drive_kwh` from `todays_event` rather than the static `expected_daily_miles`. If `todays_event` is `None`, use `expected_daily_miles` as a fallback.

4. **Consider LogNormal for daily miles (Finding 4)**: Change `build_miles_schedule()` to use `DistributionKind::LogNormal` with parameters `mu = ln(mean^2 / sqrt(mean^2 + stddev^2))`, `sigma = sqrt(ln(1 + stddev^2 / mean^2))`. This produces strictly positive, right-skewed draws that better match empirical trip distance distributions.

5. **Remove or integrate `arrival_fuzz_minutes`/`departure_fuzz_minutes` (Finding 5)**: Either delete these dead fields from `ArchetypePreset` or integrate them into the departure/arrival time sampling.

6. **Allow range anxiety override on non-driving days (Finding 6)**: Move the range anxiety check before the `todays_event.is_none()` early return in `decide()`, or restructure so that the driver can charge on non-driving days when SOC is critically low.

7. **Use configured charging efficiency (Finding 8)**: Pass the equipment's configured `charging_efficiency` into `build_preferences()` instead of hardcoding 0.9.

## References / Citations
- NREL EVI-Pro assumptions: https://afdc.energy.gov/evi-pro-lite/load-profile/assumptions (cited by OCHRE EV.py line 9)
- AAA 2019 EV range testing (5 vehicles, HVAC): cited in `efficiency.rs:16`
- Geotab 2020 fleet data (5.2M trips): cited in `efficiency.rs:17`
- DOE/Argonne 2024 cold-weather study: cited in `efficiency.rs:18`
- Recurrent Auto range loss data (30k vehicles): cited in `efficiency.rs:19`
