# EV driver archetype presets: log-normal distributions vs NHTS data
**Review ID**: dercat-05
**Category**: der-catalog
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/ev/catalog.rs` (1041 lines)
- `crates/hares-types/src/schedule.rs` (lines 170-265, 630-791; distributions, sampling, clamping)
- `docs/equipment/ev.md` (83 lines; user-facing archetype documentation)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/EV.py` (371 lines): OCHRE's EV model uses EVI-Pro joint PDF data for arrival time, SOC, and parking duration — event-based sampling from empirical data, not parametric distributions. No log-normal usage. No driver archetype concept (differentiated only by vehicle type + capacity tier).
- `vendors/OCHRE/ochre/Equipment/EventBasedLoad.py` (449 lines): Base class for event-based stochastic scheduling framework.
- `vendors/EnergyPlus/src/EnergyPlus/`: No EV charging or electric vehicle equipment model exists. The string "EVChargingStation" appears only as an internal mass surface name in unit tests (`SurfaceGeometry.unit.cc`), not as an actual EV model.

## Findings

### Finding 1: [Severity: critical] Distribution type mismatch — Gaussian used where log-normal is needed for daily miles
**Description**: The HARES documentation at `docs/equipment/ev.md:33` states: "Stochastic daily driving with log-normal mileage distribution." However, all 12 archetype presets use `DistributionKind::Gaussian` (normal distribution) for daily miles via `build_miles_schedule()` at `catalog.rs:484-495`. NHTS data shows daily miles have strong right skew (mean ~30 mi/day); a log-normal distribution is the appropriate model because a Gaussian with these parameters produces non-negligible negative values (~0.17% for DailyCommuterL2 at mean=38, std=13).

The `DistributionKind::LogNormal` variant exists in the type system (`schedule.rs:180-182`) and has proper sampling (`schedule.rs:237-239`) and analytical mean (`schedule.rs:260-261`), but it is never referenced by any archetype preset. This is a design drift: the type system supports log-normal, the documentation claims log-normal, but the archetypes only emit Gaussian.

This also affects departure time and trip duration, which use Gaussian despite time-of-day distributions being inherently bounded. While departure-time clamping to [0, 1439] is nearly lossless (Gaussian at 480±30 has ~1e-146 mass outside bounds), trip duration at mean=600±60 with clamp [30, 1200] will accumulate probability mass at the 30-minute floor.

**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:484-495` (Gaussian miles), `crates/hares-equipment/src/ev/catalog.rs:499-513` (Gaussian departure), `crates/hares-equipment/src/ev/catalog.rs:517-529` (Gaussian duration); `docs/equipment/ev.md:33` (misleading documentation claim).

**Root Cause**: The `ArchetypePreset` struct (`catalog.rs:401-427`) stores `daily_drive_miles_mean` and `daily_drive_miles_stddev` as Gaussian parameters. To use log-normal, these would need to be `daily_drive_miles_mu` and `daily_drive_miles_sigma` (log-space parameters), and the `build_miles_schedule()` method would need to emit `DistributionKind::LogNormal`. The stored parameter names (`mean`/`stddev`) force a Gaussian interpretation and prevent log-normal usage.

**Impact**: 
- Daily miles distribution does not match NHTS right-skew shape; negative-value draws (~0.17%) are clamped to 0, creating an unrealistic spike at 0 miles that does not exist in survey data.
- Fleet-level aggregate charging profiles will have the wrong statistical shape, affecting load-duration curves and peak-demand estimates.
- Documentation is false advertising — users who select archetypes expecting log-normal behavior get Gaussian instead.

---

### Finding 2: [Severity: high] Near-duplicate archetypes waste simulation diversity — DailyCommuterL2 and TouOptimizerCa are behaviorally identical
**Description**: The `DailyCommuterL2` (`catalog.rs:539-563`) and `TouOptimizerCa` (`catalog.rs:792-815`) presets have 100% identical behavioral parameters:

| Parameter | DailyCommuterL2 | TouOptimizerCa |
|-----------|----------------|----------------|
| daily_drive_miles_mean | 38.0 | 38.0 |
| daily_drive_miles_stddev | 13.0 | 13.0 |
| daily_drive_miles_min | 0.0 | 0.0 |
| departure_minute_mean | 480 (8:00) | 480 (8:00) |
| departure_minute_stddev | 30 | 30 |
| duration_minutes_mean | 600 (10h) | 600 (10h) |
| duration_minutes_stddev | 60 | 60 |
| event_day_ratio | 0.50 | 0.50 |
| arrival_fuzz_minutes | 30 | 30 |
| departure_fuzz_minutes | 30 | 30 |

The only difference is the `ChargingStrategy` variant (Nightly vs TouAware). Both archetypes will generate identical daily-mile and departure/arrival schedules. From a driving-behavior perspective, these are the same archetype, consuming two slots in the 12-entry catalog without adding behavioral diversity. OCHRE's `EV.py` avoids this by differentiating purely on vehicle class (PHEV20/50, BEV100/250) and charging level (L1/L2), not on strategy variants of identical driving patterns.

Additionally, `DailyCommuterL2` (38/13 miles) and `PhevCommuter` (35/12 miles) differ by only 3 miles mean and 1 mile stddev with identical temporal parameters. `DailyCommuterL1` (25/9) and `WorkplaceCharger` (30/10) share identical departure (480) and duration (600), differing only modestly in miles.

**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:539-563` vs `crates/hares-equipment/src/ev/catalog.rs:792-815`.

**Root Cause**: Charging strategy was used as the primary differentiator for creating separate archetype entries, but charging strategy is a configuration parameter, not a behavioral one. The presets conflate "how the vehicle is driven" with "how it charges," when these should be orthogonal dimensions.

**Impact**: Reduces the effective behavioral diversity of a 12-archetype fleet from 12 unique driving profiles to approximately 6-7 meaningfully distinct ones. Fleet simulations that sample uniformly from the catalog will over-represent the "38 mi/day, 8AM departure, 10h away" pattern.

---

### Finding 3: [Severity: high] Arrival time is not directly modeled — derived as departure+duration without independent variability
**Description**: NHTS data provides independent distributions for departure time and arrival time, each with distinct peaks (7:00-8:00 for departure, 17:00-18:00 for arrival). HARES does not model arrival time as a separate random variable. Instead, arrival time is implicitly computed as `departure_time + trip_duration` (both sampled independently). The `ArchetypePreset` struct (`catalog.rs:401-427`) has `departure_minute_mean`/`departure_minute_stddev` and `duration_minutes_mean`/`duration_minutes_stddev`, with no corresponding arrival-time parameters.

This conflates two conceptual quantities: arrival time (when the car returns home and can begin charging) is determined by departure + duration, but trip duration conflates drive time with away-from-home time. Real commuters have correlated departure and arrival times that are not captured by independent Gaussian draws of departure and duration.

For the commuter archetypes, the mean arrival works out to 480 + 600 = 1080 (6:00 PM), which is at the upper edge of the NHTS peak (17:00-18:00). The stddev of 67 minutes (√(30² + 60²)) means ~68% of arrivals fall within [16:53, 19:07], which covers the NHTS peak but with a mean that is 30 minutes late.

OCHRE's approach (`EV.py:23-24`) jointly samples arrival time, arrival SOC, and parking duration from a pre-computed joint PDF, preserving the empirical correlations. HARES loses this correlation structure.

**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:422-427` (departure and duration parameters, no arrival); `crates/hares-equipment/src/ev/catalog.rs:499-530` (independent schedules for departure and duration).

**Root Cause**: The `ArchetypePreset` and `EvDriverActor` model was designed around departure time + trip duration as the two temporal dimensions, omitting independent arrival time. This is a modeling simplification that breaks the correlation structure present in NHTS data.

**Impact**: 
- Arrival-time clustering at 18:00 ± 1h may not match empirical distributions where arrival peak is at 17:00-17:30 with smaller variance for commuters.
- The correlation between departure earliness and trip duration (longer commutes tend to start earlier) is lost when both are independent Gaussians.
- Evening charging onset timing (which depends on arrival time) is systematically shifted 30 minutes later than NHTS data would suggest.

---

### Finding 4: [Severity: medium] Simple clamping used instead of proper truncated distribution with CDF renormalization
**Description**: All stochastic schedule sources use `raw.clamp(lo, hi)` at `schedule.rs:653` for bound enforcement. This is simple clamping — values outside bounds are pinned to the boundary edge — not proper statistical truncation where the remaining probability mass within bounds is renormalized so the truncated PDF integrates to 1.0.

The code's own comments acknowledge this limitation at `schedule.rs:724-728`: "For `Stochastic`, returns the analytical distribution mean clamped to `[clamp_min, clamp_max]` -- this is an approximation when clamping is active (truncated distribution mean differs from clamped analytical mean)."

Specific cases:
- **Daily miles** (clamp_min = 0, clamp_max = None): For DailyCommuterL2 (Gaussian μ=38, σ=13), P(draw < 0) ≈ 0.0017. All ~0.17% of negative draws are clamped to 0, creating a probability spike at 0 that does not exist in NHTS data. A truncated Gaussian or log-normal would avoid this.
- **Trip duration** (clamp [30, 1200]): For DailyCommuterL2 (duration μ=600, σ=60), P(draw < 30) ≈ 0 (z = -9.5), so the 30-minute floor is not practically hit. But for WfhL1Minimal (duration μ=120, σ=30), P(draw < 30) ≈ 0.0013, and P(draw > 1200) ≈ 0 (z = 36), creating a spike at 30 minutes.
- **Departure time** (clamp [0, 1439]): For all commute archetypes with σ ≤ 60 and mean near 480, the bounds are so wide that clamping has negligible effect.

The review instructions specify the truncated PDF should integrate to 1.0. With simple clamping, the empirical distribution does not integrate to the same total probability as the untruncated distribution; instead, boundary values accumulate additional probability mass.

**Code Location**: `crates/hares-types/src/schedule.rs:641-654` (clamping logic), `crates/hares-types/src/schedule.rs:724-728` (mean approximation warning).

**Root Cause**: Simple `f64::clamp` was chosen for implementation simplicity over proper rejection sampling or inverse-CDF-based truncation.

**Impact**: For commute archetypes with Gaussian daily miles, ~0.17% of days show 0 miles when they should show small-but-positive values. For WFH/retiree archetypes with low-duration trips, the 30-minute floor accumulates probability. These boundary artifacts distort the distribution tails, which are critical for extreme-event analysis (peak charging demand days, minimum SOC events).

---

### Finding 5: [Severity: medium] Documentation claims 7 archetypes with log-normal but code implements 12 archetypes with Gaussian
**Description**: `docs/equipment/ev.md:31-43` documents 7 driver archetypes: Commuter, ShiftWorker, WorkFromHome, WeekendWarrior, SeniorRetiree, SchoolRunFamily, SingleCarShared — and states "Stochastic daily driving with log-normal mileage distribution." The actual code in `catalog.rs:538-816` defines 12 archetypes (no SchoolRunFamily, no SingleCarShared; instead DailyCommuterL2/L1, LongCommuterL2, WfhL1Minimal, HeavyUseSuv, WorkplaceCharger, PhevCommuter, TouOptimizerCa, RetireeL1). Additionally, the code uses Gaussian, not log-normal.

The `EvArchetypeId` constants in `catalog.rs:328-342` list 12 variants. `catalog.rs:935` asserts `ARCHETYPE_CATALOG.len() == 12`. The Python bindings at `crates/hares-python/src/py_enums.rs:1957-2049` expose all 12. But any user reading `docs/equipment/ev.md` will expect 7 archetypes with log-normal distributions, neither of which matches reality.

This is also a stale-documentation issue: the `SchoolRunFamily` (two daily trips) and `SingleCarShared` (shared household) archetypes described in the docs would provide useful behavioral diversity (multi-trip patterns, shared-vehicle dynamics) that are absent from the 12 implemented presets.

**Code Location**: `docs/equipment/ev.md:31-43` (stale documentation), `crates/hares-equipment/src/ev/catalog.rs:538-816` (actual 12 presets).

**Root Cause**: The archetype set was expanded during development from 7 to 12 variants, but the documentation was never updated. The log-normal claim in the docs reflects an intended design (the `DistributionKind::LogNormal` variant exists) that was never wired into the archetype presets.

**Impact**: Users relying on documentation will expect different archetypes than what is available. Researchers attempting to reproduce HARES results may be confused by the discrepancy between the published 7-archetype, log-normal design and the 12-archetype, Gaussian implementation.

---

### Finding 6: [Severity: medium] No NHTS data citations or empirical validation of distribution parameters
**Description**: The archetype parameter values at `catalog.rs:538-816` (daily miles means from 8.0 to 75.0, stddevs from 4.0 to 20.0; departure means from 360 to 600; duration means from 120 to 660) appear to be expert-estimated values. There are no NHTS data citations, no regression or fitting methodology described, and no tests that validate the generated distributions against empirical data. The test suite in `catalog.rs:818-1041` validates structural invariants (catalog ordering, parsing, strategy variants) but contains no distributional validation tests (e.g., KS test against NHTS, Q-Q plots, or even basic moment checks).

In contrast, OCHRE's `EV.py:8-9,22-24` cites EVI-Pro and the AFDC assumptions page, and samples directly from empirical joint PDF tables derived from real charging event data. While EVI-Pro is not NHTS (it's charging infrastructure data rather than travel survey data), OCHRE at least has a traceable empirical data provenance.

Additionally, none of the 12 presets match the NHTS-reported national mean of ~30 miles/day well:
- 3 presets below 30: WfhL1Minimal (8), RetireeL1 (10), DailyCommuterL1 (25)
- 3 presets near 30: ShiftWorker (30), WorkplaceCharger (30), PhevCommuter (35)
- 5 presets above 30: DailyCommuterL2 (38), TouOptimizerCa (38), HeavyUseSuv (55), LongCommuterL2 (75)
- 2 presets with weekday/weekend split: WfhOccasional (12/25), WeekendWarrior (10/30)

A weighted fleet-average with equal archetype weights yields ~33.6 miles/day — 12% above the NHTS mean. Without weighting documentation, users cannot know whether the fleet average is calibrated to NHTS.

**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:538-816` (parameter values), `crates/hares-equipment/src/ev/catalog.rs:818-1041` (test suite, no distributional tests).

**Root Cause**: The parameter values were chosen heuristically during initial authoring without a structured calibration process against NHTS data.

**Impact**: Cannot assess whether the aggregate fleet-level daily-miles distribution matches NHTS. If archetypes are used with equal weighting, the fleet mean overestimates NHTS by ~12%. High-mileage archetypes (LongCommuterL2 at 75 mi, HeavyUseSuv at 55 mi) may overstate rare-event charging demand if their prevalence in the real EV fleet is lower than 1/12 each.

---

### Finding 7: [Severity: low] Departure time means for commute archetypes cluster at 480 (8:00 AM) — NHTS peak is 7:00-8:00
**Description**: NHTS reports peak departure time at 7:00-8:00 AM for commuters. Six of the 12 HARES archetypes use `departure_minute_mean = 480` (8:00 AM), which is at the right edge of the NHTS peak window. Only one archetype (LongCommuterL2, `departure_minute_mean = 420` = 7:00 AM) lands in the center of the NHTS peak. The ShiftWorker at 360 (6:00 AM) is earlier than typical commuters. The remaining archetypes (WfhOccasional, WfhL1Minimal, WeekendWarrior, RetireeL1) have later departures (540-600), which is appropriate for non-commuting profiles.

With stddev = 30 minutes for commute archetypes, ~68% of departures fall between 7:30-8:30, meaning only the left tail overlaps the NHTS peak. A mean of 450 (7:30) with stddev 30 would center the distribution on the NHTS peak while still capturing the 8:00 departures in the right tail.

Additionally, 7 out of 12 archetypes (DailyCommuterL2, DailyCommuterL1, HeavyUseSuv, WorkplaceCharger, ShiftWorker, PhevCommuter, TouOptimizerCa) have identical or very similar departure σ = 30, meaning the temporal spread is identical across archetypes that should have different variability (a shift worker should have tighter departure schedule than a retiree).

**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:559,581,601,625,647,671,695,719,741,761,787,812` (departure_minute_mean values).

**Root Cause**: The 8:00 AM mean was likely chosen as a round number central tendency without precise NHTS peak alignment. The uniform stddev = 30 across commute archetypes suggests a copy-paste pattern rather than per-archetype calibration.

**Impact**: Morning departure peak in fleet simulations will be centered ~30 minutes later than NHTS data. This shifts the morning "home charging window" end 30 minutes later, affecting TOU rate overlap calculations and grid load timing.

---

### Finding 8: [Severity: low] Seven archetypes share identical departure-minimum (0.0) — unnecessary for archetypes intended never to have zero-mile days
**Description**: The `daily_drive_miles_min` field serves as the floor value for miles sampling (applied as `clamp_min`). Ten of 12 archetypes set `daily_drive_miles_min = 0.0`. Only `LongCommuterL2` (min=10) and `HeavyUseSuv` (min=5) enforce a positive floor. For a daily commuter who drives 38 miles on average, having the floor at 0 means Gaussian draws below zero (P≈0.0017) are clamped to 0, producing occasional zero-mile "commute" days. A floor of, say, 5 miles for DailyCommuterL2 would be more realistic — a daily commuter never drives 0 miles on a workday.

The event_day_ratio (0.50 for DailyCommuterL2, 0.70 for PhevCommuter) partially compensates by making some days non-commute days, but on event days, the floor of 0 is unrealistic.

**Code Location**: `crates/hares-equipment/src/ev/catalog.rs:553-554,574-575,596,619,643,666,690,714,735,755,781,806` (daily_drive_miles_min and daily_drive_miles_mean values).

**Root Cause**: The `daily_drive_miles_min` field defaults to 0.0 in the struct design and was not customized per archetype to match behavioral intent.

**Impact**: ~0.17% of commute-day draws for DailyCommuterL2 produce 0 miles. While small in percentage terms, across a fleet of 10,000 vehicles over 365 days, this represents ~62 days per vehicle of anomalous zero-mile commutes that would not occur in a properly calibrated commuter profile.

---

## Summary
- Total findings: 8
- Critical: 1 (Gaussian vs log-normal distribution mismatch)
- High: 2 (near-duplicate archetypes; missing independent arrival-time modeling)
- Medium: 3 (simple clamping without truncation; stale documentation; no NHTS calibration)
- Low: 2 (departure mean offset from NHTS peak; excessive zero-mile floors for commuters)

## Recommendations

1. **Switch daily-miles distributions to log-normal**. Store log-space `mu`/`sigma` in `ArchetypePreset` and emit `DistributionKind::LogNormal` in `build_miles_schedule()`. For NHTS calibration, NHTS 2017 reports daily VMT mean ≈ 29.2 mi with σ ≈ 25 mi for all vehicles; for EV owners specifically, EVI-Pro data suggests mean ≈ 35 mi/day. Fit a log-normal: μ ≈ 3.35, σ ≈ 0.65 yields mean ≈ 35 mi with right skew. Validate that the resulting distribution has a realistic 95th percentile (~80 mi) and 99th percentile (~120 mi).

2. **Merge or differentiate the near-duplicate archetypes**. Either collapse `DailyCommuterL2` and `TouOptimizerCa` into a single behavioral preset with a separate charging-strategy axis, or differentiate them behaviorally (e.g., L2 commuter at 38 mi/day vs TOU optimizer at 42 mi/day with tighter departure variance). Similarly, review `DailyCommuterL1` vs `WorkplaceCharger` (both L1, both ~30 mi, both 480 departure) — differentiate by departure time, duration, or miles.

3. **Add independent arrival-time modeling**. Add `arrival_minute_mean` and `arrival_minute_stddev` to `ArchetypePreset`. Sample arrival time directly for the plug-in event. Derive trip duration from (arrival − departure) or sample it independently for the away-from-home calculation. This restores the correlation structure present in NHTS data.

4. **Implement proper truncated distributions**. Replace `raw.clamp(lo, hi)` with rejection sampling within the target bounds, or use inverse-CDF methods with renormalized probability. Alternatively, use `rand_distr::Normal` with rejection for [0,∞) and explicitly document the truncation approach. For log-normal daily miles, natural [0,∞) support eliminates the negative-value problem entirely.

5. **Update docs/equipment/ev.md** to reflect the 12 actual archetypes (or the reduced set after merging duplicates), correct the distribution type claim, and document the NHTS calibration methodology. If the SchoolRunFamily and SingleCarShared archetypes are deferred, note them as future work.

6. **Add NHTS calibration tests** that validate: (a) fleet-weighted mean daily miles ≤ 32 mi (within 10% of NHTS), (b) departure-time distribution peaks at 7:00-8:00 for commuter archetypes, (c) arrival-time distribution peaks at 17:00-18:00 for commuter archetypes, (d) no archetype produces negative miles (log-normal fix), (e) KS test p > 0.05 against NHTS binned distributions for the aggregate fleet.

7. **Adjust departure_minute_mean for commuter archetypes** from 480 (8:00 AM) to 450 (7:30 AM) to center on the NHTS peak, and vary stddev per archetype (commuter: σ=20-25 for tighter schedule, retiree: σ=60-90 for looser schedule) rather than the uniform σ=30 used across 7 archetypes.

8. **Set realistic daily_drive_miles_min floors** for commuter archetypes: DailyCommuterL2 (min=5), PhevCommuter (min=3), WorkplaceCharger (min=3), LongCommuterL2 (already min=10). Non-commuter archetypes like WfhOccasional and RetireeL1 can keep min=0.

## References / Citations

- **NHTS 2017 (National Household Travel Survey)**: FHWA. Mean daily VMT ≈ 29.2 miles; departure peak 7:00-8:00; arrival peak 17:00-18:00. Right-skewed distribution unsuitable for symmetric Gaussian.
- **EVI-Pro (NREL)**: https://afdc.energy.gov/evi-pro-lite/load-profile/assumptions — OCHRE's reference for EV charging event probabilities; uses joint PDF of arrival time, SOC, and parking duration from empirical charging data.
- **OCHRE EV.py** (`vendors/OCHRE/ochre/Equipment/EV.py:22-24`): Event-based model using EVI-Pro joint PDF; differentiates by vehicle type + capacity tier, not driver archetypes. No log-normal usage.
- **HARES schedule.rs** (`crates/hares-types/src/schedule.rs:180-182`): `DistributionKind::LogNormal` variant exists but is unused by archetypes.
- **HARES catalog.rs** (`crates/hares-equipment/src/ev/catalog.rs:538-816`): All 12 archetype presets with Gaussian distribution parameters.
- **HARES ev.md** (`docs/equipment/ev.md:31-43`): Documents 7 archetypes with claimed log-normal — stale and inaccurate.
